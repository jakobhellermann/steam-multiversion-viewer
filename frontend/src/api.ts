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

/** Progress of an in-flight interactive login (mirrors backend `LoginPhase`). */
export type LoginPhase =
  | { phase: "starting" }
  | { phase: "waiting_device" }
  | { phase: "need_code"; code_type: string; details: string; device_available: boolean }
  | { phase: "error"; message: string };

export type AuthStatus = {
  authenticated: boolean;
  /** Steam account name, present while logged in or while a login is pending. */
  account?: string;
  /** Steam3 id (e.g. `[U:1:...]`), present only while logged in. */
  steamid?: string;
  /** Set while an interactive login is in progress. */
  pending?: LoginPhase | null;
};

export function fetchAuthStatus(): Promise<AuthStatus> {
  return getJson("/api/auth/status");
}

export async function login(credentials: {
  account: string;
  password: string;
}): Promise<AuthStatus> {
  const r = await fetch("/api/auth/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(credentials),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

/**
 * Submit a Steam Guard code. Confirming in the mobile app needs no call —
 * the backend detects that out-of-band and the login completes on its own.
 */
export async function submitLoginCode(code: string): Promise<AuthStatus> {
  const r = await fetch("/api/auth/login/code", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export async function logout(): Promise<AuthStatus> {
  const r = await fetch("/api/auth/logout", { method: "POST" });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
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
  bytes_total_compressed: number;
  bytes_missing_compressed: number;
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

export type ManifestDiffStatus = "added" | "changed";

export type ManifestDiffEntry = {
  path: string;
  status: ManifestDiffStatus;
};

export async function fetchManifestDiff(
  appid: AppId,
  base: ManifestRef,
  others: ManifestRef[],
): Promise<ManifestDiffEntry[]> {
  if (others.length === 0) return [];
  const r = await fetch(`/api/apps/${appid}/manifests/diff`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ base, others }),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  const body: { entries: ManifestDiffEntry[] } = await r.json();
  return body.entries;
}

/// Deep variant of `fetchManifestDiff`: 1:1 against a single target,
/// and `changed` files whose *structured* diff is empty are dropped.
/// Far slower — the backend downloads both sides of every changed file.
export async function fetchManifestDiffDeep(
  appid: AppId,
  base: ManifestRef,
  target: ManifestRef,
): Promise<ManifestDiffEntry[]> {
  const qs = new URLSearchParams({
    branch: base.branch,
    target_depot_id: String(target.depot_id),
    target_manifest_id: target.manifest_id,
    target_branch: target.branch,
  });
  const r = await fetch(
    `/api/apps/${appid}/depots/${base.depot_id}/manifests/${base.manifest_id}/structured-diff-filter?${qs}`,
  );
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  const body: { entries: ManifestDiffEntry[] } = await r.json();
  return body.entries;
}

/// `{depot_id, manifest_id}` of a target whose at-least-one-path-
/// matching-query differs from base. Returned by
/// `/manifests/diff-targets` (see [`fetchManifestDiffTargets`]).
export type ManifestDiffTarget = {
  depot_id: number;
  manifest_id: string;
};

/// Filter a candidate list of `others` down to those manifests where
/// at least one path matching `query` (whitespace-AND-token substring,
/// case-insensitive) differs from base. Used by the compare-to menu
/// on the manifest-detail page so the dropdown only lists targets
/// where the user's current search has changes to show.
export async function fetchManifestDiffTargets(
  appid: AppId,
  base: ManifestRef,
  others: ManifestRef[],
  query: string,
): Promise<ManifestDiffTarget[]> {
  if (others.length === 0) return [];
  const r = await fetch(`/api/apps/${appid}/manifests/diff-targets`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ base, others, query }),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  const body: { matching_targets: ManifestDiffTarget[] } = await r.json();
  return body.matching_targets;
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
  /// Which rich view (if any) this file gets — mutually exclusive.
  rich_view: RichView | null;
};

export type RichView = "transformed" | "structured";

export type StructuredNode = {
  id: string;
  label: string;
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
  /// Set on diff-tree nodes; absent for plain (non-diff) structured
  /// trees. The renderer colours the row when present.
  status?: NodeStatus;
  /// When true, the content endpoint serves a per-node body for this
  /// row (a JSON dump, a decompiled type, …). Absent on rows that
  /// only group or summarise — frontend skips the content fetch and
  /// shows a placeholder.
  has_content?: boolean;
  /// Body MIME when known up front and not plain text (e.g. `"image/png"` for a texture).
  content_mime?: string;
  children: StructuredNode[];
};

export type StructuredTree = {
  root: StructuredNode;
};

export type NodeContent = {
  mime: string;
  text: string;
};

export type NodeStatus = "unchanged" | "changed" | "added" | "removed";

/// Fetch the structured diff for a file between two manifests.
/// Returns the same shape as `fetchFileStructured`, with `status` and
/// optional `object_ref` set on nodes the diff touched. Returns null
/// on HTTP 415 (caller should fall back to the textual diff).
export async function fetchStructuredDiff(
  appid: AppId,
  baseDepotId: number,
  baseManifestId: string,
  baseBranch: string,
  target: { depot_id: number; manifest_id: string; branch: string },
  path: string,
): Promise<StructuredTree | null> {
  const qs = new URLSearchParams({
    path,
    branch: baseBranch,
    target_depot_id: String(target.depot_id),
    target_manifest_id: target.manifest_id,
    target_branch: target.branch,
  });
  const r = await fetch(
    `/api/apps/${appid}/depots/${baseDepotId}/manifests/${baseManifestId}/file/structured-diff?${qs}`,
  );
  if (r.status === 415) return null;
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export type StructuredDiffNodeContent = {
  /// Content-Type of the body, without parameters: `text/x-diff` for a
  /// two-sided unified diff, otherwise whatever the one available side
  /// dumped as. Stays a mime rather than a grammar so this module does
  /// not pull in the highlighter.
  mime: string;
  text: string;
};

/// Fetch the per-node body for a structured diff entry. Backend runs
/// the JSON dump on each side and the text-diff in one shot. `nodeId`
/// is the raw tree-row id — the backend parses out which side(s) to
/// dump from its prefix (`base:`, `target:`, `mod:`, or no prefix).
export async function fetchStructuredDiffNode(
  appid: AppId,
  baseDepotId: number,
  baseManifestId: string,
  baseBranch: string,
  target: { depot_id: number; manifest_id: string; branch: string },
  path: string,
  nodeId: string,
): Promise<StructuredDiffNodeContent> {
  const qs = new URLSearchParams({
    path,
    branch: baseBranch,
    target_depot_id: String(target.depot_id),
    target_manifest_id: target.manifest_id,
    target_branch: target.branch,
    node_id: nodeId,
  });
  const r = await fetch(
    `/api/apps/${appid}/depots/${baseDepotId}/manifests/${baseManifestId}/file/structured-diff/node?${qs}`,
  );
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  const ct = (r.headers.get("content-type") ?? "").split(";")[0].trim();
  const text = await r.text();
  return { mime: ct, text };
}

/// URL for a structured node's image body (a Texture2D as PNG).
export function structuredNodeImageUrl(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
  path: string,
  nodeId: string,
): string {
  const qs = new URLSearchParams({ branch, path, node_id: nodeId });
  return `/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/file/structured/node/image?${qs}`;
}

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
  const qs = new URLSearchParams({ branch, path, node_id: nodeId });
  const r = await fetch(
    `/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/file/structured/node?${qs}`,
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

export type EngineInfo = {
  engine: "unity";
  data: { version: string; bundle_version?: string };
};
export type GameInfo = { engine: EngineInfo | null };

export function fetchGameInfo(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
): Promise<GameInfo> {
  const qs = new URLSearchParams({ branch });
  return getJson(`/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/game_info?${qs}`);
}

export type VibrancyEffect = "none" | "mica" | "acrylic";

export type Config = {
  store_root: string;
  mountpoint: string;
  export_dir: string;
  vibrancy_effect: VibrancyEffect;
  vibrancy_tint: number;
  restart_required: boolean;
};

export function fetchConfig(): Promise<Config> {
  return getJson("/api/config");
}

export async function patchConfig(patch: {
  store_root?: string;
  mountpoint?: string;
  export_dir?: string;
  vibrancy_effect?: VibrancyEffect;
  vibrancy_tint?: number;
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

/// Windows-only: `startMount` may report that ProjFS wasn't enabled and a
/// UAC prompt to enable it was shown. `enabled` / `restart_needed` mean the
/// user should mount again; `cancelled` means they dismissed the prompt.
export type ProjfsEnableOutcome = "enabled" | "restart_needed" | "cancelled";

export type StartMountResult =
  | { kind: "mounted"; mountpoint: string }
  | { kind: "projfs_prompt"; outcome: ProjfsEnableOutcome };

export async function startMount(): Promise<StartMountResult> {
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

export type ExportState = "idle" | "running" | "done" | "failed" | "cancelled";

export type ExportTarget = {
  app_id: number;
  depot_id: number;
  manifest_id: string;
};

export type ExportStatus = {
  state: ExportState;
  target: ExportTarget | null;
  target_dir: string | null;
  files_total: number;
  files_done: number;
  bytes_total: number;
  bytes_written: number;
  current_path: string | null;
  error: string | null;
};

export async function exportManifest(
  appid: AppId,
  depotId: number,
  manifestId: string,
  body: { branch: string; subdir?: string },
): Promise<ExportStatus> {
  const r = await fetch(`/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/export`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export function fetchExportStatus(): Promise<ExportStatus> {
  return getJson("/api/export");
}

export async function cancelExport(): Promise<ExportStatus> {
  const r = await fetch("/api/export/cancel", { method: "POST" });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
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

export type StoreManifest = {
  manifest_id: string;
  creation_time: number;
  chunks_total: number;
  chunks_present: number;
  bytes_total: number;
  bytes_on_disk: number;
  bytes_unique: number;
};

export type StoreDepot = {
  depot_id: number;
  manifests: StoreManifest[];
};

export type StoreApp = {
  app_id: AppId;
  bytes_on_disk: number;
  depots: StoreDepot[];
};

export type StoreOverview = {
  total_bytes_on_disk: number;
  total_chunks_on_disk: number;
  unreferenced: { chunks: number; bytes: number };
  apps: StoreApp[];
};

export type StoreManifestRef = {
  app_id: AppId;
  depot_id: number;
  manifest_id: string;
};

export type PruneRequest = {
  free_chunks?: StoreManifestRef[];
  delete_metadata?: StoreManifestRef[];
  include_unreferenced?: boolean;
};

export type PruneResult = { freed_bytes: number; freed_chunks: number };

export function fetchStore(): Promise<StoreOverview> {
  return getJson("/api/store");
}

export async function prunePreview(body: PruneRequest): Promise<PruneResult> {
  const r = await fetch("/api/store/prune/preview", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export async function prune(body: PruneRequest): Promise<PruneResult> {
  const r = await fetch("/api/store/prune", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}
