// TODO(ai-review): review for style and correctness
//! Steam library + per-app manifest metadata.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::json;
use steam_vent::ConnectionTrait;
use steam_vent_depot::FileType;
use steam_vent_proto::steammessages_player_steamclient::CPlayer_GetOwnedGames_Request;
use tokio::sync::Semaphore;
use utoipa::{IntoParams, ToSchema};

use crate::http::{ApiError, CacheSeconds, ImmutableCache};
use crate::routes::extra_manifests::{ExtraManifestEntryDto, extra_manifests_of};
use crate::state::store_index::ManifestStats;
use crate::state::{AppState, Snapshot};
use crate::steam::{AppId, DepotId, ManifestId};

use super::{Result, default_branch};

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "appid": 620,
    "name": "Portal 2",
    "playtime_minutes": 1051
}))]
pub struct OwnedGame {
    pub appid: AppId,
    pub name: String,
    pub playtime_minutes: u32,
}

/// Steam library list
#[utoipa::path(
    get,
    path = "/api/library",
    tag = "library",
    responses((status = 200, body = Vec<OwnedGame>))
)]
#[tracing::instrument(skip_all)]
pub async fn library(
    State(state): State<AppState>,
) -> Result<(CacheSeconds, Json<Vec<OwnedGame>>)> {
    let games = fetch_owned_games(&state).await?;
    // Owned-games changes are rare (new purchase, new playtime); 5min cache.
    Ok((CacheSeconds(300), Json(games)))
}

/// Fetches the owned-games list directly from the Steam connection.
/// Shared between the HTTP handler and the native menu bar.
pub async fn fetch_owned_games(state: &AppState) -> Result<Vec<OwnedGame>> {
    let steam = state.steam()?;
    let req = CPlayer_GetOwnedGames_Request {
        steamid: Some(steam.connection.steam_id().into()),
        include_appinfo: Some(true),
        include_played_free_games: Some(true),
        ..Default::default()
    };

    let resp = steam.connection.service_method(req).await?;

    let games: Vec<OwnedGame> = resp
        .games
        .into_iter()
        .map(|g| OwnedGame {
            appid: AppId(g.appid() as u32),
            name: g.name().to_string(),
            playtime_minutes: g.playtime_forever() as u32,
        })
        .collect();
    state
        .disk_cache
        .write(&library_cache_key(&steam.account), &games)?;
    Ok(games)
}

/// Disk-cache key of an account's owned games as last fetched from Steam.
pub fn library_cache_key(account: &str) -> String {
    format!("library/{account}.json")
}

#[derive(Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "appid": 1145360,
    "name": "Hades",
    "type": "Game",
    "developer": "Supergiant Games",
    "publisher": "Supergiant Games",
    "homepage": "http://www.supergiantgames.com",
    "logo_url": "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/1145360/79416954db9fc5d5079c26839a77cd35b09e1608.jpg",
    "icon_url": "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/1145360/8a3fca36a00883e8066263ad35dd15d77a1f9abc.jpg",
    "branches": [
        {"name": "public", "build_id": 10929685, "time_updated": 1681309217, "description": null}
    ],
    "depots": [
        {
            "depot_id": 1145361,
            "oslist": "windows",
            "osarch": null,
            "language": "",
            "from_app_id": null,
            "manifests": [
                {"branch": "public", "manifest_id": "5173085471687919135", "size": 11406885121u64, "download_size": 10694072752u64}
            ]
        }
    ],
    "private_branches": true
}))]
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

#[derive(Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "name": "public",
    "build_id": 10929685,
    "time_updated": 1681309217,
    "description": null
}))]
pub struct BranchInfo {
    pub name: String,
    pub build_id: u64,
    pub time_updated: Option<u64>,
    pub description: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "depot_id": 1234567,
    "oslist": "windows",
    "osarch": null,
    "language": "",
    "from_app_id": null,
    "manifests": [
        {"branch": "public", "manifest_id": "9876543210987654321", "size": 11406885121u64, "download_size": 10694072752u64}
    ]
}))]
pub struct DepotEntry {
    pub depot_id: DepotId,
    pub oslist: Option<String>,
    pub osarch: Option<String>,
    pub language: Option<String>,
    /// If set, this depot's content actually lives under a different app, e.g. Steamworks Common Redistributables
    pub from_app_id: Option<AppId>,
    pub manifests: Vec<DepotManifest>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "branch": "public",
    "manifest_id": "9876543210987654321",
    "size": 11406885121u64,
    "download_size": 10694072752u64
}))]
pub struct DepotManifest {
    pub branch: String,
    pub manifest_id: ManifestId,
    pub size: u64,
    pub download_size: u64,
}

/// App metadata
#[utoipa::path(
    get,
    path = "/api/apps/{appid}",
    tag = "library",
    responses((status = 200, body = AppInfo))
)]
#[tracing::instrument(skip_all)]
pub async fn app_info(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
) -> Result<(CacheSeconds, Json<AppInfo>)> {
    // App metadata is pretty stable; let the browser cache it for 5 minutes
    // before re-asking.
    Ok((
        CacheSeconds(300),
        Json(fetch_app_info(&state, appid).await?),
    ))
}

/// App info as last fetched, or fetched from Steam when there is none yet.
pub async fn app_info_or_fetch(state: &AppState, appid: AppId) -> Result<AppInfo> {
    match state.disk_cache.read(&app_info_cache_key(appid))? {
        Some((info, _)) => Ok(info),
        None => fetch_app_info(state, appid).await,
    }
}

/// App info from Steam, persisted as the last fetched copy.
pub async fn fetch_app_info(state: &AppState, appid: AppId) -> Result<AppInfo> {
    let info = state.steam()?.depot.app_info(appid.0).await?;

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

    let app = AppInfo {
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
    };
    state.disk_cache.write(&app_info_cache_key(appid), &app)?;
    Ok(app)
}

/// Every tracked manifest of the app (app-info heads, then user-tracked extras), once per
/// `(depot, gid)` with the first branch it was seen on.
pub fn app_manifest_refs(info: &AppInfo, extras: &[ExtraManifestEntryDto]) -> Vec<ManifestRef> {
    let mut seen = HashSet::new();
    let heads = info.depots.iter().flat_map(|depot| {
        depot.manifests.iter().map(|m| ManifestRef {
            depot_id: depot.depot_id,
            manifest_id: m.manifest_id,
            branch: m.branch.clone(),
        })
    });
    let tracked = extras.iter().map(|e| ManifestRef {
        depot_id: e.depot_id,
        manifest_id: e.manifest_id,
        branch: e.branch.clone().unwrap_or_else(default_branch),
    });
    heads
        .chain(tracked)
        .filter(|r| seen.insert((r.depot_id, r.manifest_id)))
        .collect()
}

/// Disk-cache key of an app's info as last fetched from Steam.
pub fn app_info_cache_key(appid: AppId) -> String {
    format!("app_info/{appid}.json")
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
#[schema(example = json!({
    "depot_id": 1234567,
    "manifest_id": "9876543210987654321",
    "creation_time": 1680271331,
    "size_uncompressed": 11406885121u64,
    "size_compressed": 10694072752u64,
    "file_count": 4619
}))]
pub struct ManifestInfo {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub creation_time: u32,
    pub size_uncompressed: u64,
    pub size_compressed: u64,
    pub file_count: usize,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "depot_id": 1234567,
    "manifest_id": "9876543210987654321",
    "files": [
        {"path": "Data/Engine.dll",       "size": 9437184,  "kind": "file",    "chunk_count": 10, "linktarget": null},
        {"path": "Data/example/asset.bin", "size": 62451264, "kind": "file",    "chunk_count": 61, "linktarget": null},
        {"path": "Data/empty.txt",         "size": 0,        "kind": "file",    "chunk_count": 0,  "linktarget": null},
        {"path": "Data/lib.so",            "size": 0,        "kind": "symlink", "chunk_count": 0,  "linktarget": "lib.so.1"}
    ]
}))]
pub struct ManifestFiles {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub files: Vec<ManifestFile>,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "path": "Data/example/asset.bin",
    "size": 62451264,
    "kind": "file",
    "chunk_count": 61,
    "linktarget": null
}))]
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

impl From<FileType> for ManifestFileKind {
    fn from(value: FileType) -> Self {
        match value {
            FileType::File => Self::File,
            FileType::Directory => Self::Directory,
            FileType::Symlink => Self::Symlink,
        }
    }
}

/// Manifest header
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}",
    tag = "library",
    params(ManifestInfoQuery),
    responses((status = 200, body = ManifestInfo))
)]
#[tracing::instrument(skip_all)]
pub async fn manifest_info(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<ManifestInfoQuery>,
) -> Result<(ImmutableCache, Json<ManifestInfo>)> {
    state.steam()?; // 401 if not logged in
    let snapshot = state
        .open_manifest(appid, depot_id, manifest_id, &q.branch)
        .await?;
    Ok((ImmutableCache, Json(manifest_info_of(&snapshot))))
}

pub fn manifest_info_of(snapshot: &Snapshot) -> ManifestInfo {
    let m = snapshot.manifest();
    ManifestInfo {
        depot_id: DepotId(m.depot_id),
        manifest_id: ManifestId(m.manifest_id),
        creation_time: m.creation_time,
        size_uncompressed: m.size_uncompressed,
        size_compressed: m.size_compressed,
        // Directories are implicit in file paths, so don't count
        // them — keeps the value consistent with the listing route.
        file_count: m.files.iter().filter(|f| !f.is_dir()).count(),
    }
}

/// Manifest file list
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/files",
    tag = "library",
    params(ManifestFilesQuery),
    responses((status = 200, body = ManifestFiles))
)]
#[tracing::instrument(skip_all)]
pub async fn manifest_files(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<ManifestFilesQuery>,
) -> Result<(ImmutableCache, Json<ManifestFiles>)> {
    state.steam()?; // 401 if not logged in
    let snapshot = state
        .open_manifest(appid, depot_id, manifest_id, &q.branch)
        .await?;
    Ok((ImmutableCache, Json(manifest_files_of(&snapshot))))
}

pub fn manifest_files_of(snapshot: &Snapshot) -> ManifestFiles {
    let m = snapshot.manifest();
    // Directories are implicit in file paths — skipping them keeps the
    // listing scannable.
    let files = m
        .files
        .iter()
        .filter(|f| !f.is_dir())
        .map(|f| ManifestFile {
            path: f.path.clone(),
            size: f.size,
            kind: f.file_type().into(),
            chunk_count: f.chunks().len() as u32,
            linktarget: f.linktarget().map(str::to_owned),
        })
        .collect();

    ManifestFiles {
        depot_id: DepotId(m.depot_id),
        manifest_id: ManifestId(m.manifest_id),
        files,
    }
}

#[derive(Deserialize, ToSchema)]
pub struct ManifestStatusRequest {
    pub manifests: Vec<ManifestRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ManifestRef {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    /// Needed to mint a manifest-request-code if the manifest isn't cached yet.
    /// For already-cached entries this is ignored.
    #[serde(default = "default_branch")]
    pub branch: String,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "depot_id": 1234567,
    "manifest_id": "9876543210987654321",
    "error": null,
    "chunks_total": 14586,
    "chunks_missing": 13583,
    "bytes_total": 11283371566u64,
    "bytes_total_compressed": 10694072752u64,
    "bytes_missing_compressed": 9716479376u64,
    "creation_time": 1680271331
}))]
pub struct ManifestStatusEntry {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    /// Set when this manifest couldn't be opened (e.g. restricted branch
    /// the account has no license for). Other stats are zeroed.
    pub error: Option<String>,
    pub chunks_total: u32,
    pub chunks_missing: u32,
    pub bytes_total: u64,
    /// Compressed bytes — total download size of the whole manifest.
    pub bytes_total_compressed: u64,
    /// Compressed bytes that would be pulled over the wire to complete the download.
    pub bytes_missing_compressed: u64,
    /// Manifest creation time (Steam-side timestamp, unix seconds). Lets
    /// callers sort tracked manifests chronologically without having to
    /// open each one again. Zero when `error` is set.
    pub creation_time: u32,
}

/// Manifest status batch
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/status",
    tag = "library",
    request_body = ManifestStatusRequest,
    responses((status = 200, body = Vec<ManifestStatusEntry>))
)]
#[tracing::instrument(skip_all, fields(count = body.manifests.len()))]
pub async fn manifest_statuses(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ManifestStatusRequest>,
) -> Result<Json<Vec<ManifestStatusEntry>>> {
    Ok(Json(statuses_of(&state, appid, &body.manifests).await?))
}

/// Status of every tracked manifest of the app, see [`app_manifest_refs`]
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/manifests/status",
    tag = "library",
    responses((status = 200, body = Vec<ManifestStatusEntry>))
)]
#[tracing::instrument(skip_all)]
pub async fn app_manifest_statuses(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
) -> Result<Json<Vec<ManifestStatusEntry>>> {
    let info = app_info_or_fetch(&state, appid).await?;
    let refs = app_manifest_refs(&info, &extra_manifests_of(&state, appid));
    Ok(Json(statuses_of(&state, appid, &refs).await?))
}

/// Statuses of the `refs` the built store model holds, without opening any manifest, and
/// whether that covers all of them.
pub fn indexed_statuses(
    state: &AppState,
    appid: AppId,
    refs: &[ManifestRef],
) -> Option<(Vec<ManifestStatusEntry>, bool)> {
    let model = state.built_store_model()?;
    let index = state.store_index.read().expect("store_index poisoned");
    let statuses: Vec<_> = refs
        .iter()
        .filter_map(|r| {
            let (stats, creation_time) =
                model.manifest_stats((appid, r.depot_id, r.manifest_id), index.present_chunks())?;
            Some(status_entry(
                r.depot_id,
                r.manifest_id,
                stats,
                creation_time,
            ))
        })
        .collect();
    let complete = statuses.len() == refs.len();
    Some((statuses, complete))
}

async fn statuses_of(
    state: &AppState,
    appid: AppId,
    manifests: &[ManifestRef],
) -> Result<Vec<ManifestStatusEntry>> {
    state.steam()?; // 401 if not logged in
    // Dedupe — multiple branches often share a manifest_id, no point asking
    // the cache (or the CDN) for it twice in one request.
    let mut seen = HashSet::new();
    let refs: Vec<&ManifestRef> = manifests
        .iter()
        .filter(|r| seen.insert((r.depot_id, r.manifest_id)))
        .collect();

    // Manifests on disk come from the in-memory store model; only the rest are opened.
    let model = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || state.store_model())
            .await
            .map_err(|e| ApiError::internal(format!("store model task panicked: {e}")))??
    };
    let mut out = Vec::new();
    let mut unindexed = Vec::new();
    {
        let index = state.store_index.read().expect("store_index poisoned");
        for r in refs {
            match model.manifest_stats((appid, r.depot_id, r.manifest_id), index.present_chunks()) {
                Some((stats, creation_time)) => out.push(status_entry(
                    r.depot_id,
                    r.manifest_id,
                    stats,
                    creation_time,
                )),
                None => unindexed.push(r),
            }
        }
    }

    // Cap parallelism — even with mostly-cached calls, eight concurrent
    // file-reads is plenty. For the cold path, this caps CDN connections.
    let sem = Arc::new(Semaphore::new(8));
    let mut fu = FuturesUnordered::new();
    for r in unindexed {
        let sem = sem.clone();
        fu.push(async move {
            let _permit = sem.acquire().await.expect("semaphore not closed");
            let result = state
                .open_manifest(appid, r.depot_id, r.manifest_id, &r.branch)
                .await;
            (r.depot_id, r.manifest_id, result)
        });
    }

    while let Some((depot_id, manifest_id, result)) = fu.next().await {
        let entry = match result {
            Ok(snap) => manifest_status_of(state, &snap),
            Err(err) => {
                tracing::warn!(%depot_id, %manifest_id, %err, "manifest open failed");
                ManifestStatusEntry {
                    depot_id,
                    manifest_id,
                    error: Some(err.to_string()),
                    chunks_total: 0,
                    chunks_missing: 0,
                    bytes_total: 0,
                    bytes_total_compressed: 0,
                    bytes_missing_compressed: 0,
                    creation_time: 0,
                }
            }
        };
        out.push(entry);
    }

    Ok(out)
}

pub fn manifest_status_of(state: &AppState, snapshot: &Snapshot) -> ManifestStatusEntry {
    let manifest = snapshot.manifest();
    let stats = state
        .store_index
        .read()
        .expect("store_index poisoned")
        .manifest_stats(manifest);
    status_entry(
        DepotId(manifest.depot_id),
        ManifestId(manifest.manifest_id),
        stats,
        manifest.creation_time,
    )
}

fn status_entry(
    depot_id: DepotId,
    manifest_id: ManifestId,
    stats: ManifestStats,
    creation_time: u32,
) -> ManifestStatusEntry {
    ManifestStatusEntry {
        depot_id,
        manifest_id,
        error: None,
        chunks_total: stats.chunks_total,
        chunks_missing: stats.chunks_missing,
        bytes_total: stats.bytes_total,
        bytes_total_compressed: stats.bytes_total_compressed,
        bytes_missing_compressed: stats.bytes_missing_compressed,
        creation_time,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(branch: &str, gid: u64) -> DepotManifest {
        DepotManifest {
            branch: branch.to_owned(),
            manifest_id: ManifestId(gid),
            size: 0,
            download_size: 0,
        }
    }

    #[test]
    fn app_manifest_refs_dedupe_with_first_branch_and_public_default() {
        let info = AppInfo {
            appid: AppId(1),
            name: String::new(),
            r#type: String::new(),
            developer: None,
            publisher: None,
            homepage: None,
            logo_url: None,
            icon_url: String::new(),
            branches: Vec::new(),
            depots: vec![DepotEntry {
                depot_id: DepotId(2),
                oslist: None,
                osarch: None,
                language: None,
                from_app_id: None,
                manifests: vec![
                    manifest("public", 10),
                    manifest("beta", 10),
                    manifest("beta", 11),
                ],
            }],
            private_branches: false,
        };
        let extras = [
            ExtraManifestEntryDto {
                depot_id: DepotId(2),
                manifest_id: ManifestId(11),
                branch: Some("old".to_owned()),
            },
            ExtraManifestEntryDto {
                depot_id: DepotId(2),
                manifest_id: ManifestId(12),
                branch: None,
            },
        ];
        let refs: Vec<(u64, String)> = app_manifest_refs(&info, &extras)
            .into_iter()
            .map(|r| (r.manifest_id.0, r.branch))
            .collect();
        assert_eq!(
            refs,
            [
                (10, "public".to_owned()),
                (11, "beta".to_owned()),
                (12, "public".to_owned())
            ]
        );
    }
}
