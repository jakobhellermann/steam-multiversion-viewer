use axum::Json;
use axum::extract::{Path, Query, State};
use camino::Utf8PathBuf;
use futures_util::stream::TryStreamExt;
use serde::{Deserialize, Serialize};
use steam_vent::ConnectionTrait;
use steam_vent_depot::FileKind;
use steam_vent_proto::steammessages_player_steamclient::CPlayer_GetOwnedGames_Request;
use utoipa::{IntoParams, ToSchema};

use crate::config::Config;
use crate::error::ApiError;
use crate::http::ImmutableCache;
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

type Result<T, E = ApiError> = std::result::Result<T, E>;

#[derive(Serialize, ToSchema)]
pub struct OwnedGame {
    pub appid: AppId,
    pub name: String,
    pub playtime_minutes: u32,
}

#[utoipa::path(get, path = "/api/library")]
pub async fn library(State(state): State<AppState>) -> Result<Json<Vec<OwnedGame>>> {
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

    Ok(Json(games))
}

#[derive(Serialize, ToSchema)]
pub struct AppInfo {
    pub appid: AppId,
    pub name: String,
    pub r#type: String,
    pub developer: String,
    pub publisher: String,
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
    pub manifests: Vec<DepotManifest>,
}

#[derive(Serialize, ToSchema)]
pub struct DepotManifest {
    pub branch: String,
    pub gid: ManifestId,
    pub size: u64,
    pub download_size: u64,
}

#[utoipa::path(get, path = "/api/apps/{appid}")]
#[tracing::instrument(skip(state))]
pub async fn app_info(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
) -> Result<Json<AppInfo>> {
    let info = state.steam.depot.app_info(appid.0).await?;

    let asset_url = |hash: &str, ext: &str| {
        format!(
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{appid}/{hash}.{ext}"
        )
    };

    let mut branches: Vec<BranchInfo> = info
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
    // Stable order: public first, then by name.
    branches.sort_by(|a, b| match (a.name.as_str(), b.name.as_str()) {
        ("public", "public") => std::cmp::Ordering::Equal,
        ("public", _) => std::cmp::Ordering::Less,
        (_, "public") => std::cmp::Ordering::Greater,
        (l, r) => l.cmp(r),
    });

    let mut depot_ids: Vec<u32> = info.depots.depots.keys().copied().collect();
    depot_ids.sort();
    let depots = depot_ids
        .into_iter()
        .map(|id| {
            let d = &info.depots.depots[&id];
            let mut manifests: Vec<DepotManifest> = d
                .manifests
                .iter()
                .map(|(branch, m)| DepotManifest {
                    branch: branch.clone(),
                    gid: ManifestId(m.gid),
                    size: m.size,
                    download_size: m.download,
                })
                .collect();
            manifests.sort_by(|a, b| a.branch.cmp(&b.branch));
            DepotEntry {
                depot_id: DepotId(id),
                oslist: d.config.as_ref().and_then(|c| c.oslist.clone()),
                osarch: d.config.as_ref().and_then(|c| c.osarch.clone()),
                language: d.config.as_ref().and_then(|c| c.language.clone()),
                manifests,
            }
        })
        .collect();

    Ok(Json(AppInfo {
        appid,
        name: info.common.name,
        r#type: info.common.r#type,
        developer: info.extended.developer,
        publisher: info.extended.publisher,
        homepage: info.extended.homepage,
        logo_url: info.common.logo.as_deref().map(|h| asset_url(h, "jpg")),
        icon_url: asset_url(&info.common.icon, "jpg"),
        branches,
        depots,
        private_branches: info.depots.private_branches,
    }))
}

fn default_branch() -> String {
    "public".into()
}

fn default_limit() -> usize {
    100
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
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
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
pub struct ManifestFilesPage {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub offset: usize,
    pub limit: usize,
    pub file_count: usize,
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
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{gid}",
    params(ManifestInfoQuery)
)]
#[tracing::instrument(skip(state))]
pub async fn manifest_info(
    State(state): State<AppState>,
    Path((appid, depot_id, gid)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<ManifestInfoQuery>,
) -> Result<(ImmutableCache, Json<ManifestInfo>)> {
    let snapshot = state.open_manifest(appid, depot_id, gid, &q.branch).await?;
    let m = snapshot.manifest();

    Ok((
        ImmutableCache,
        Json(ManifestInfo {
            depot_id: DepotId(m.depot_id),
            manifest_id: ManifestId(m.manifest_id),
            creation_time: m.creation_time,
            size_uncompressed: m.size_uncompressed,
            size_compressed: m.size_compressed,
            file_count: m.files.len(),
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{gid}/files",
    params(ManifestFilesQuery)
)]
#[tracing::instrument(skip(state))]
pub async fn manifest_files(
    State(state): State<AppState>,
    Path((appid, depot_id, gid)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<ManifestFilesQuery>,
) -> Result<(ImmutableCache, Json<ManifestFilesPage>)> {
    let snapshot = state.open_manifest(appid, depot_id, gid, &q.branch).await?;
    let m = snapshot.manifest();

    let total = m.files.len();
    let start = q.offset.min(total);
    let end = start.saturating_add(q.limit).min(total);

    let files = m.files[start..end]
        .iter()
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
        Json(ManifestFilesPage {
            depot_id: DepotId(m.depot_id),
            manifest_id: ManifestId(m.manifest_id),
            offset: start,
            limit: end - start,
            file_count: total,
            files,
        }),
    ))
}

#[derive(Serialize, ToSchema)]
pub struct ConfigDto {
    #[schema(value_type = String)]
    pub store_root: Utf8PathBuf,
    pub restart_required: bool,
}

/// Partial config update — only fields the client wants to change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchConfig {
    #[schema(value_type = String)]
    pub store_root: Option<Utf8PathBuf>,
}

fn build_config_dto(state: &AppState, saved: Config) -> ConfigDto {
    ConfigDto {
        restart_required: saved.store_root != state.config.store_root,
        store_root: saved.store_root,
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
    cfg.save()?;
    Ok(Json(build_config_dto(&state, cfg)))
}

#[derive(Serialize, ToSchema)]
pub struct ManifestStatusEntry {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub branch: String,
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
}

#[utoipa::path(get, path = "/api/apps/{appid}/manifests/status")]
#[tracing::instrument(skip(state))]
pub async fn manifest_statuses(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
) -> Result<Json<Vec<ManifestStatusEntry>>> {
    let info = state.steam.depot.app_info(appid.0).await?;

    // Multiple branches may share the same gid, resulting in an unnecessary
    // open_manifest call.
    let mut fu = futures_util::stream::FuturesUnordered::new();
    for (&depot_id, depot) in &info.depots.depots {
        let depot_id = DepotId(depot_id);
        for (branch, m) in &depot.manifests {
            let branch = branch.clone();
            let gid = ManifestId(m.gid);
            let state = &state;
            fu.push(async move {
                let snap = state.open_manifest(appid, depot_id, gid, &branch).await?;
                Ok::<_, ApiError>((depot_id, branch, snap))
            });
        }
    }
    let mut out = Vec::new();
    while let Some((depot_id, branch, snap)) = fu.try_next().await? {
        let manifest = snap.manifest();
        let stats = state.store_index.read().await.manifest_stats(manifest);
        out.push(ManifestStatusEntry {
            depot_id,
            manifest_id: ManifestId(manifest.manifest_id),
            branch,
            chunks_total: stats.chunks_total,
            chunks_missing: stats.chunks_missing,
            bytes_total: stats.bytes_total,
            bytes_missing: stats.bytes_missing,
            bytes_missing_compressed: stats.bytes_missing_compressed,
            bytes_unique: stats.bytes_unique,
        });
    }

    Ok(Json(out))
}
