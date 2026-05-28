use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use camino::Utf8PathBuf;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
use steam_vent::ConnectionTrait;
use steam_vent_depot::FileKind;
use steam_vent_proto::steammessages_player_steamclient::CPlayer_GetOwnedGames_Request;
use tokio::sync::Semaphore;
use utoipa::{IntoParams, ToSchema};

use crate::config::Config;
use crate::error::ApiError;
use crate::http::{CacheSeconds, ImmutableCache};
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

pub mod diff;
pub mod downloads;
pub mod mount;
pub mod structured;

pub(crate) type Result<T, E = ApiError> = std::result::Result<T, E>;

#[derive(Serialize, ToSchema)]
pub struct OwnedGame {
    pub appid: AppId,
    pub name: String,
    pub playtime_minutes: u32,
}

#[utoipa::path(get, path = "/api/library")]
#[tracing::instrument(skip_all)]
pub async fn library(
    State(state): State<AppState>,
) -> Result<(CacheSeconds, Json<Vec<OwnedGame>>)> {
    let req = CPlayer_GetOwnedGames_Request {
        steamid: Some(state.steam.connection.steam_id().into()),
        include_appinfo: Some(true),
        include_played_free_games: Some(true),
        ..Default::default()
    };

    let resp = state.steam.connection.service_method(req).await?;

    let games = resp
        .games
        .into_iter()
        .map(|g| OwnedGame {
            appid: AppId(g.appid() as u32),
            name: g.name().to_string(),
            playtime_minutes: g.playtime_forever() as u32,
        })
        .collect();

    // Owned-games changes are rare (new purchase, new playtime); 5min cache.
    Ok((CacheSeconds(300), Json(games)))
}

#[derive(Serialize, ToSchema)]
pub struct AppInfo {
    pub appid: AppId,
    pub name: String,
    pub r#type: String,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub homepage: Option<String>,
    pub logo_url: Option<String>,
    pub icon_url: String,
    pub branches: Vec<BranchInfo>,
    pub depots: Vec<DepotEntry>,
    pub private_branches: bool,
}

#[derive(Serialize, ToSchema)]
pub struct BranchInfo {
    pub name: String,
    pub build_id: u64,
    pub time_updated: Option<u64>,
    pub description: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DepotEntry {
    pub depot_id: DepotId,
    pub oslist: Option<String>,
    pub osarch: Option<String>,
    pub language: Option<String>,
    /// If set, this depot's content actually lives under a different app, e.g. Steamworks Common Redistributables
    pub from_app_id: Option<AppId>,
    pub manifests: Vec<DepotManifest>,
}

#[derive(Serialize, ToSchema)]
pub struct DepotManifest {
    pub branch: String,
    pub manifest_id: ManifestId,
    pub size: u64,
    pub download_size: u64,
}

#[utoipa::path(get, path = "/api/apps/{appid}")]
#[tracing::instrument(skip_all)]
pub async fn app_info(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
) -> Result<(CacheSeconds, Json<AppInfo>)> {
    let info = state.steam.depot.app_info(appid.0).await?;

    let asset_url = |hash: &str, ext: &str| {
        format!(
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{appid}/{hash}.{ext}"
        )
    };

    let branches: Vec<BranchInfo> = info
        .depots
        .branches
        .iter()
        .map(|(name, b)| BranchInfo {
            name: name.clone(),
            build_id: b.build_id,
            time_updated: b.time_updated,
            description: b.description.clone(),
        })
        .collect();

    let depots: Vec<DepotEntry> = info
        .depots
        .depots
        .iter()
        .map(|(&id, d)| {
            let manifests: Vec<DepotManifest> = d
                .manifests
                .iter()
                .map(|(branch, m)| DepotManifest {
                    branch: branch.clone(),
                    manifest_id: ManifestId(m.gid),
                    size: m.size,
                    download_size: m.download,
                })
                .collect();
            DepotEntry {
                depot_id: DepotId(id),
                oslist: d.config.as_ref().and_then(|c| c.oslist.clone()),
                osarch: d.config.as_ref().and_then(|c| c.osarch.clone()),
                language: d.config.as_ref().and_then(|c| c.language.clone()),
                from_app_id: d.depot_from_app.map(AppId),
                manifests,
            }
        })
        .collect();

    // App metadata is prett ystable; let the browser cache it for 5 minutes
    // before re-asking.
    Ok((
        CacheSeconds(300),
        Json(AppInfo {
            appid,
            name: info.common.name,
            r#type: info.common.r#type,
            developer: info.extended.as_ref().map(|e| e.developer.clone()),
            publisher: info.extended.as_ref().map(|e| e.publisher.clone()),
            homepage: info.extended.and_then(|e| e.homepage),
            logo_url: info.common.logo.as_deref().map(|h| asset_url(h, "jpg")),
            icon_url: asset_url(&info.common.icon, "jpg"),
            branches,
            depots,
            private_branches: info.depots.private_branches,
        }),
    ))
}

pub(super) fn default_branch() -> String {
    "public".into()
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ManifestInfoQuery {
    #[serde(default = "default_branch")]
    pub branch: String,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ManifestFilesQuery {
    #[serde(default = "default_branch")]
    pub branch: String,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestInfo {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub creation_time: u32,
    pub size_uncompressed: u64,
    pub size_compressed: u64,
    pub file_count: usize,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestFiles {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub files: Vec<ManifestFile>,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub kind: ManifestFileKind,
    pub chunk_count: u32,
    pub linktarget: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ManifestFileKind {
    File,
    Directory,
    Symlink,
}

impl From<FileKind> for ManifestFileKind {
    fn from(value: FileKind) -> Self {
        match value {
            FileKind::File => Self::File,
            FileKind::Directory => Self::Directory,
            FileKind::Symlink => Self::Symlink,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}",
    params(ManifestInfoQuery)
)]
#[tracing::instrument(skip_all)]
pub async fn manifest_info(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<ManifestInfoQuery>,
) -> Result<(ImmutableCache, Json<ManifestInfo>)> {
    let snapshot = state
        .open_manifest(appid, depot_id, manifest_id, &q.branch)
        .await?;
    let m = snapshot.manifest();

    Ok((
        ImmutableCache,
        Json(ManifestInfo {
            depot_id: DepotId(m.depot_id),
            manifest_id: ManifestId(m.manifest_id),
            creation_time: m.creation_time,
            size_uncompressed: m.size_uncompressed,
            size_compressed: m.size_compressed,
            // Directories are implicit in file paths, so don't count
            // them — keeps the value consistent with the listing route.
            file_count: m
                .files
                .iter()
                .filter(|f| !matches!(f.kind, FileKind::Directory))
                .count(),
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/files",
    params(ManifestFilesQuery)
)]
#[tracing::instrument(skip_all)]
pub async fn manifest_files(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<ManifestFilesQuery>,
) -> Result<(ImmutableCache, Json<ManifestFiles>)> {
    let snapshot = state
        .open_manifest(appid, depot_id, manifest_id, &q.branch)
        .await?;
    let m = snapshot.manifest();

    // Directories are implicit in file paths — skipping them keeps the
    // listing scannable.
    let files = m
        .files
        .iter()
        .filter(|f| !matches!(f.kind, FileKind::Directory))
        .map(|f| ManifestFile {
            path: f.path.clone(),
            size: f.size,
            kind: f.kind.into(),
            chunk_count: f.chunks.len() as u32,
            linktarget: f.linktarget.clone(),
        })
        .collect();

    Ok((
        ImmutableCache,
        Json(ManifestFiles {
            depot_id: DepotId(m.depot_id),
            manifest_id: ManifestId(m.manifest_id),
            files,
        }),
    ))
}

#[derive(Serialize, ToSchema)]
pub struct ConfigDto {
    #[schema(value_type = String)]
    pub store_root: Utf8PathBuf,
    #[schema(value_type = String)]
    pub mountpoint: Utf8PathBuf,
    pub restart_required: bool,
}

/// Partial config update — only fields the client wants to change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchConfig {
    #[schema(value_type = String)]
    pub store_root: Option<Utf8PathBuf>,
    #[schema(value_type = String)]
    pub mountpoint: Option<Utf8PathBuf>,
}

fn build_config_dto(state: &AppState, saved: Config) -> ConfigDto {
    ConfigDto {
        // Only `store_root` requires a restart — it's baked into
        // `DepotStore` at init. `mountpoint` is live-reloaded on the
        // next `/api/mount/start`.
        restart_required: saved.store_root != state.initial_config.store_root,
        store_root: saved.store_root,
        mountpoint: saved.mountpoint,
    }
}

#[utoipa::path(get, path = "/api/config")]
pub async fn get_config(State(state): State<AppState>) -> Result<Json<ConfigDto>> {
    let saved = Config::load_or_default()?;
    Ok(Json(build_config_dto(&state, saved)))
}

#[utoipa::path(patch, path = "/api/config", request_body = PatchConfig)]
pub async fn patch_config(
    State(state): State<AppState>,
    Json(body): Json<PatchConfig>,
) -> Result<Json<ConfigDto>> {
    let mut cfg = Config::load_or_default()?;
    if let Some(store_root) = body.store_root {
        // Create the dir on save so users see "saved" only when the path is
        // actually usable. The running process keeps using the old root until
        // restart.
        std::fs::create_dir_all(&store_root)?;
        cfg.store_root = store_root;
    }
    if let Some(mountpoint) = body.mountpoint {
        // Only validated when the user actually starts the mount; we
        // intentionally don't create the dir here.
        cfg.mountpoint = mountpoint;
    }
    cfg.save()?;
    // Publish the new config to every other request handler atomically.
    state.config.store(Arc::new(cfg.clone()));
    Ok(Json(build_config_dto(&state, cfg)))
}

#[derive(Deserialize, ToSchema)]
pub struct ManifestStatusRequest {
    pub manifests: Vec<ManifestRef>,
}

#[derive(Deserialize, ToSchema)]
pub struct ManifestRef {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    /// Needed to mint a manifest-request-code if the manifest isn't cached yet.
    /// For already-cached entries this is ignored.
    #[serde(default = "default_branch")]
    pub branch: String,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestStatusEntry {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    /// Set when this manifest couldn't be opened (e.g. restricted branch
    /// the account has no license for). Other stats are zeroed.
    pub error: Option<String>,
    pub chunks_total: u32,
    pub chunks_missing: u32,
    pub bytes_total: u64,
    /// Additional uncompressed bytes needed on disk to fully download this manifest.
    pub bytes_missing: u64,
    /// Compressed bytes that would be pulled over the wire to complete the download.
    pub bytes_missing_compressed: u64,
    /// Bytes currently on disk that are only referenced by this manifest
    /// (i.e. what you'd reclaim by deleting it).
    pub bytes_unique: u64,
    /// Manifest creation time (Steam-side timestamp, unix seconds). Lets
    /// callers sort tracked manifests chronologically without having to
    /// open each one again. Zero when `error` is set.
    pub creation_time: u32,
}

/// Batch status for the listed manifests. Cached manifests are returned
/// immediately; uncached ones get fetched from the Steam CDN, which can be
/// slow on the first call to a fresh app but is fast on subsequent ones.
/// The `branch` field is only used during the CDN fetch — for cached
/// entries any value (e.g. "public") works.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/status",
    request_body = ManifestStatusRequest
)]
#[tracing::instrument(skip_all, fields(count = body.manifests.len()))]
pub async fn manifest_statuses(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ManifestStatusRequest>,
) -> Result<Json<Vec<ManifestStatusEntry>>> {
    // Dedupe — multiple branches often share a manifest_id, no point asking
    // the cache (or the CDN) for it twice in one request.
    let mut seen = HashSet::new();
    let refs: Vec<&ManifestRef> = body
        .manifests
        .iter()
        .filter(|r| seen.insert((r.depot_id, r.manifest_id)))
        .collect();

    // Cap parallelism — even with mostly-cached calls, eight concurrent
    // file-reads is plenty. For the cold path, this caps CDN connections.
    let sem = Arc::new(Semaphore::new(8));
    let mut fu = FuturesUnordered::new();
    for r in refs {
        let state = &state;
        let sem = sem.clone();
        fu.push(async move {
            let _permit = sem.acquire().await.expect("semaphore not closed");
            let result = state
                .open_manifest(appid, r.depot_id, r.manifest_id, &r.branch)
                .await;
            (r.depot_id, r.manifest_id, result)
        });
    }

    let mut out = Vec::new();
    while let Some((depot_id, manifest_id, result)) = fu.next().await {
        let entry = match result {
            Ok(snap) => {
                let manifest = snap.manifest();
                let stats = state
                    .store_index
                    .read()
                    .expect("store_index poisoned")
                    .manifest_stats(manifest);
                ManifestStatusEntry {
                    depot_id,
                    manifest_id,
                    error: None,
                    chunks_total: stats.chunks_total,
                    chunks_missing: stats.chunks_missing,
                    bytes_total: stats.bytes_total,
                    bytes_missing: stats.bytes_missing,
                    bytes_missing_compressed: stats.bytes_missing_compressed,
                    bytes_unique: stats.bytes_unique,
                    creation_time: manifest.creation_time,
                }
            }
            Err(err) => {
                tracing::warn!(%depot_id, %manifest_id, %err, "manifest open failed");
                ManifestStatusEntry {
                    depot_id,
                    manifest_id,
                    error: Some(err.to_string()),
                    chunks_total: 0,
                    chunks_missing: 0,
                    bytes_total: 0,
                    bytes_missing: 0,
                    bytes_missing_compressed: 0,
                    bytes_unique: 0,
                    creation_time: 0,
                }
            }
        };
        out.push(entry);
    }

    Ok(Json(out))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FileViewQuery {
    #[serde(default = "default_branch")]
    pub branch: String,
    pub path: String,
    /// When false, skip the chunk download and inline-content fetch and
    /// just return metadata (size, sha, kind, chunks_present). The
    /// default is true so existing callers keep their auto-fetch
    /// behavior. Used by the compare menu to peek at all candidate
    /// shas without warming hundreds of files.
    #[serde(default = "default_true")]
    pub auto_fetch: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, ToSchema)]
pub struct FileView {
    pub path: String,
    pub size: u64,
    pub kind: ManifestFileKind,
    pub chunk_count: u32,
    pub chunks_present: u32,
    pub linktarget: Option<String>,
    /// Steam-side content sha1 of the file, hex-encoded. Stable identity
    /// for "is this the same file?" comparisons across manifests. None
    /// when the manifest doesn't carry one (e.g. symlinks).
    pub sha: Option<String>,
    /// SHA-256-ish guess: "text" | "binary" | "unknown" (kind != File).
    pub content_kind: FileContentKind,
    /// Inline UTF-8 content for small text files. `None` means "too large
    /// to preview" or "not a text-like extension". The caller decides
    /// whether to offer a fetch/download instead.
    pub content: Option<String>,
    /// When `content` is `None` because of a size cap, the cap that was
    /// applied (so the caller can surface "this file is X MiB, cap is
    /// Y MiB" without hardcoding the limit).
    pub preview_cap_bytes: u64,
    /// Set when a text transformer is registered for this file's
    /// extension — callers should treat it as "the `/file/transformed`
    /// endpoint will yield text for this file" rather than guessing
    /// from the extension themselves.
    pub transformer: Option<TransformerInfo>,
    /// Set when the backend can build a [`::transform::structured::StructuredTree`]
    /// for this file. The frontend uses it to decide whether to render
    /// the tree view instead of (or alongside) the plain preview.
    pub structured: Option<StructuredInfo>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct TransformerInfo {
    /// MIME type of the transformer's output (e.g. `text/x-csharp`).
    pub mime: String,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct StructuredInfo {
    /// Renderer hint matching `StructuredTree::kind` (e.g.
    /// `"unity-serialized"`).
    pub kind: String,
}

#[derive(Serialize, ToSchema, Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FileContentKind {
    Text,
    Binary,
    /// Directory / symlink / anything not a regular file.
    Unknown,
    /// File is larger than the preview cap; we didn't read it, so we
    /// can't tell text from binary.
    TooLarge,
}

/// Maximum size we'll auto-fetch for inline preview. Above this we just
/// return metadata so a click on a 2 GiB asset doesn't silently warm
/// hundreds of chunks.
const PREVIEW_CAP_BYTES: u64 = 64 * 1024 * 1024;

/// Content-based text/binary heuristic: a NUL byte in the first sniff
/// window is a hard "binary" signal. Otherwise the bytes must parse as
/// UTF-8 to count as text.
fn looks_like_text(bytes: &[u8]) -> bool {
    const SNIFF: usize = 8192;
    let head = &bytes[..bytes.len().min(SNIFF)];
    if head.contains(&0) {
        return false;
    }
    std::str::from_utf8(bytes).is_ok()
}

#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file",
    params(FileViewQuery)
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<Json<FileView>> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    // Extract everything we need from the borrowed manifest before
    // handing the snapshot Arc to the download manager.
    let (
        file_path,
        file_size,
        file_kind,
        file_linktarget,
        file_sha,
        chunk_count,
        chunks_for_dl,
        chunk_shas,
    ) = {
        let manifest = snapshot.manifest();
        let file = manifest
            .files
            .iter()
            .find(|f| f.path == q.path)
            .ok_or_else(|| ApiError {
                status: axum::http::StatusCode::NOT_FOUND,
                message: format!("file not in manifest: {}", q.path),
            })?;
        (
            file.path.clone(),
            file.size,
            ManifestFileKind::from(file.kind),
            file.linktarget.clone(),
            file.sha.map(hex_encode),
            file.chunks.len() as u32,
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
            file.chunks.iter().map(|c| c.sha).collect::<Vec<_>>(),
        )
    };

    let (content_kind, content) = if !q.auto_fetch {
        // Caller wants metadata only — skip the download + content read.
        // We don't classify text-vs-binary without reading the bytes,
        // so return `Unknown`.
        (FileContentKind::Unknown, None)
    } else {
        match file_kind {
            ManifestFileKind::File if file_size > PREVIEW_CAP_BYTES => {
                (FileContentKind::TooLarge, None)
            }
            ManifestFileKind::File => {
                // Route the fetch through the DownloadManager so the live
                // drawer reflects the load and the chunks-present index is
                // kept consistent. `enqueue_and_wait` returns once every
                // chunk has either landed on disk (and been recorded in the
                // index) or failed; `read_full` after that is a pure cache
                // hit.
                state
                    .downloads
                    .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
                    .await;
                let bytes = snapshot.read_full(&file_path).await?;
                if looks_like_text(&bytes) {
                    match String::from_utf8(bytes.to_vec()) {
                        Ok(s) => (FileContentKind::Text, Some(s)),
                        Err(_) => (FileContentKind::Binary, None),
                    }
                } else {
                    (FileContentKind::Binary, None)
                }
            }
            _ => (FileContentKind::Unknown, None),
        }
    };

    let chunks_present = {
        let index = state.store_index.read().expect("store_index poisoned");
        chunk_shas.iter().filter(|sha| index.has_chunk(sha)).count() as u32
    };

    let transformer = ::transform::tools::transformer_for(&file_path).map(|t| TransformerInfo {
        mime: t.output_mime().to_string(),
    });
    let structured = structured_info_for(&file_path);

    Ok(Json(FileView {
        path: file_path,
        size: file_size,
        kind: file_kind,
        chunk_count,
        chunks_present,
        linktarget: file_linktarget,
        sha: file_sha,
        content_kind,
        content,
        preview_cap_bytes: PREVIEW_CAP_BYTES,
        transformer,
        structured,
    }))
}

/// Probe whether the backend has a structured-tree builder for this
/// file. Keeps the route layer out of feature-cfg territory.
fn structured_info_for(path: &str) -> Option<StructuredInfo> {
    use ::transform::Transformer;
    match ::transform::tools::transformer_for(path) {
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized | Transformer::UnityBundle) => Some(StructuredInfo {
            kind: ::transform::unity::serializedfile::tree::TREE_KIND.to_string(),
        }),
        Some(Transformer::Dll) => Some(StructuredInfo {
            kind: ::transform::dll::tree::TREE_KIND.to_string(),
        }),
        _ => None,
    }
}

fn hex_encode(bytes: [u8; 20]) -> String {
    let mut s = String::with_capacity(40);
    for b in bytes {
        use std::fmt::Write as _;
        write!(&mut s, "{b:02x}").expect("write to String");
    }
    s
}

#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/raw",
    params(FileViewQuery)
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_raw(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<Response> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    let (file_path, file_kind, chunks_for_dl) = {
        let manifest = snapshot.manifest();
        let file = manifest
            .files
            .iter()
            .find(|f| f.path == q.path)
            .ok_or_else(|| ApiError {
                status: axum::http::StatusCode::NOT_FOUND,
                message: format!("file not in manifest: {}", q.path),
            })?;
        (
            file.path.clone(),
            ManifestFileKind::from(file.kind),
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
        )
    };

    if !matches!(file_kind, ManifestFileKind::File) {
        return Err(ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: format!("not a file: {file_path}"),
        });
    }

    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;
    // PERF: read_full buffers the entire file in memory before we send a
    // byte. Fine for the typical image/audio asset, but a 2 GiB bundle
    // would blow up here. Switch to a streaming reader on DepotSnapshot
    // and pipe it into an axum Body once the use case shows up.
    let bytes = snapshot.read_full(&file_path).await?;

    let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
    Ok((
        ImmutableCache,
        [(header::CONTENT_TYPE, mime.as_ref().to_string())],
        bytes.to_vec(),
    )
        .into_response())
}

#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/transformed",
    params(FileViewQuery)
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_transformed(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<Response> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    let (file_path, file_sha, chunks_for_dl) = {
        let manifest = snapshot.manifest();
        let file = manifest
            .files
            .iter()
            .find(|f| f.path == q.path)
            .ok_or_else(|| ApiError {
                status: axum::http::StatusCode::NOT_FOUND,
                message: format!("file not in manifest: {}", q.path),
            })?;
        let sha = file.sha.ok_or_else(|| ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: format!("file has no content sha: {}", q.path),
        })?;
        (
            file.path.clone(),
            sha,
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
        )
    };

    let transformer = ::transform::tools::transformer_for(&file_path).ok_or_else(|| ApiError {
        status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        message: format!("no transformer for {file_path}"),
    })?;

    let content_type = format!("{}; charset=utf-8", transformer.output_mime());

    let cfg = state.config.load();
    // Cache hit short-circuits the (potentially expensive) tool run.
    if let Some(cached) = ::transform::read_cached(&cfg.store_root, &file_sha)? {
        return Ok((
            ImmutableCache,
            [(header::CONTENT_TYPE, content_type.clone())],
            cached,
        )
            .into_response());
    }

    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;
    let text = match transformer {
        ::transform::Transformer::Cli(tool) => {
            let bytes = snapshot.read_full(&file_path).await?;
            ::transform::run_and_cache(&cfg.store_root, tool, &file_sha, &bytes)
                .await
                .map_err(|e| ApiError {
                    status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    message: e.to_string(),
                })?
        }
        #[cfg(feature = "unity")]
        ::transform::Transformer::UnitySerialized => {
            run_unity_dump(snapshot.clone(), file_path.clone()).await?
        }
        ::transform::Transformer::Dll => {
            return Err(ApiError {
                status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                message:
                    ".NET assemblies are served through /file/structured, not /file/transformed"
                        .to_string(),
            });
        }
        #[cfg(feature = "unity")]
        ::transform::Transformer::UnityBundle => {
            return Err(ApiError {
                status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                message: "Unity bundles are served through /file/structured, not /file/transformed"
                    .to_string(),
            });
        }
    };

    Ok((ImmutableCache, [(header::CONTENT_TYPE, content_type)], text).into_response())
}

/// Run the synchronous rabex dump off the async worker — its inner
/// `block_in_place`/`block_on` plumbing makes it unsafe to call
/// directly from an async handler.
#[cfg(feature = "unity")]
async fn run_unity_dump(
    snapshot: Arc<crate::state::Snapshot>,
    path: String,
) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || ::transform::unity::dump_unity_serialized(snapshot, &path))
        .await
        .map_err(|e| ApiError {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("unity dump task panicked: {e}"),
        })?
        .map_err(|e| ApiError {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        })
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ExtraManifestEntryDto {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub branch: Option<String>,
}

impl From<crate::extra_manifests::ExtraManifestEntry> for ExtraManifestEntryDto {
    fn from(e: crate::extra_manifests::ExtraManifestEntry) -> Self {
        Self {
            depot_id: e.depot_id,
            manifest_id: e.manifest_id,
            branch: e.branch,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct ExtraManifestsRequest {
    pub entries: Vec<ExtraManifestEntryDto>,
}

impl From<ExtraManifestEntryDto> for crate::extra_manifests::ExtraManifestEntry {
    fn from(e: ExtraManifestEntryDto) -> Self {
        Self {
            depot_id: e.depot_id,
            manifest_id: e.manifest_id,
            branch: e.branch,
        }
    }
}

#[utoipa::path(get, path = "/api/apps/{appid}/extra_manifests")]
pub async fn get_extra_manifests(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
) -> Result<Json<Vec<ExtraManifestEntryDto>>> {
    let list = state.extra_manifests.list(appid);
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

/// Replace the full list of user-tracked manifests for this app.
#[utoipa::path(
    put,
    path = "/api/apps/{appid}/extra_manifests",
    request_body = ExtraManifestsRequest
)]
pub async fn put_extra_manifests(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ExtraManifestsRequest>,
) -> Result<Json<Vec<ExtraManifestEntryDto>>> {
    let entries = body.entries.into_iter().map(Into::into).collect();
    let saved = state.extra_manifests.set(appid, entries)?;
    Ok(Json(saved.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    delete,
    path = "/api/apps/{appid}/extra_manifests/{depot_id}/{manifest_id}"
)]
pub async fn delete_extra_manifest(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
) -> Result<Json<Vec<ExtraManifestEntryDto>>> {
    let saved = state.extra_manifests.remove(appid, depot_id, manifest_id)?;
    Ok(Json(saved.into_iter().map(Into::into).collect()))
}
