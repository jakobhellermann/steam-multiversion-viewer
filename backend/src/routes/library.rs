// TODO(ai-review): review for style and correctness
//! Steam library + per-app manifest metadata.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
use steam_vent::ConnectionTrait;
use steam_vent_depot::FileKind;
use steam_vent_proto::steammessages_player_steamclient::CPlayer_GetOwnedGames_Request;
use tokio::sync::Semaphore;
use utoipa::{IntoParams, ToSchema};

use crate::http::{CacheSeconds, ImmutableCache};
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

use super::{Result, default_branch};

#[derive(Serialize, ToSchema)]
pub struct OwnedGame {
    pub appid: AppId,
    pub name: String,
    pub playtime_minutes: u32,
}

#[utoipa::path(get, path = "/api/library", tag = "library")]
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

#[utoipa::path(get, path = "/api/apps/{appid}", tag = "library")]
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

    // App metadata is pretty stable; let the browser cache it for 5 minutes
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
    tag = "library",
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
    tag = "library",
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
    tag = "library",
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
