// TODO(ai-review): review for style and correctness
//! `/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/game_info`
//! — engine + engine-version detection for a manifest. Currently only
//! Unity (version from `globalgamemanagers`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[cfg(feature = "unity")]
use crate::http::ApiError;
use crate::state::{AppState, Snapshot};
use crate::steam::{AppId, DepotId, ManifestId};

use super::Result;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct GameInfoQuery {
    #[serde(default = "crate::routes::default_branch")]
    pub branch: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(tag = "engine", content = "data", rename_all = "snake_case")]
pub enum EngineInfo {
    Unity {
        /// Engine version (e.g. "2022.3.62f1"). Pulled from
        /// `globalgamemanagers`' file header — present on every
        /// Unity build.
        version: String,
        /// `PlayerSettings.bundleVersion` — the developer-set
        /// shipping version string (e.g. "1.0.5"). `None` if the
        /// PlayerSettings object is absent or carries no value
        /// (older Unity versions can leave this empty).
        #[serde(skip_serializing_if = "Option::is_none")]
        bundle_version: Option<String>,
    },
}

/// Minimal `PlayerSettings` shape — we only deserialise the one
/// field we care about. The typetree carries dozens of others that
/// we'd skip anyway, so a slim shape keeps the deserialise cheap and
/// resilient to upstream additions.
#[cfg(feature = "unity")]
#[derive(Debug, serde::Deserialize)]
#[allow(non_snake_case)]
struct PlayerSettingsSlim {
    bundleVersion: Option<String>,
}

#[cfg(feature = "unity")]
impl rabex_env::rabex::objects::ClassIdType for PlayerSettingsSlim {
    const CLASS_ID: rabex_env::rabex::objects::ClassId =
        rabex_env::rabex::objects::ClassId::PlayerSettings;
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct GameInfo {
    /// `null` when no supported engine was detected.
    pub engine: Option<EngineInfo>,
}

/// Engine detection for a manifest
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/game_info",
    tag = "library",
    params(GameInfoQuery),
    responses((status = 200, body = GameInfo))
)]
#[tracing::instrument(skip_all)]
pub async fn game_info(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<GameInfoQuery>,
) -> Result<Json<GameInfo>> {
    // Not browser-cached although immutable: requests must reach the backend so the result
    // lands in the disk cache the preload reads.
    let key = cache_key(depot_id, manifest_id);
    if let Some((cached, _)) = state.disk_cache.read(&key)? {
        return Ok(Json(cached));
    }
    let info = detect(&state, appid, depot_id, manifest_id, &q.branch).await?;
    // Without the unity feature every manifest reads as engine-less, which must not stick.
    if cfg!(feature = "unity") {
        state.disk_cache.write(&key, &info)?;
    }
    Ok(Json(info))
}

/// Disk-cache key of a manifest's game info; immutable like the manifest itself.
pub fn cache_key(depot_id: DepotId, manifest_id: ManifestId) -> String {
    format!("game_info/{depot_id}/{manifest_id}.json")
}

async fn detect(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
) -> Result<GameInfo> {
    state.steam()?; // 401 if not logged in
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, branch)
            .await?,
    );
    detect_engine(state, appid, depot_id, manifest_id, branch, snapshot).await
}

#[cfg(feature = "unity")]
async fn detect_engine(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    snapshot: Arc<Snapshot>,
) -> Result<GameInfo> {
    {
        let scratch = state
            .manifest_cache
            .scratch(appid, depot_id, manifest_id, branch);
        let Some(unity) = scratch.unity(snapshot) else {
            return Ok(GameInfo { engine: None });
        };
        let env = unity.env.clone();

        // Both fields come out of globalgamemanagers — combine the
        // reads in one blocking call so we don't pay the deserialise
        // setup twice. PlayerSettings is best-effort: a missing or
        // unparsable object should not turn the whole game-info into
        // a 500, it just means we have no bundleVersion to report.
        let (version, bundle_version) = tokio::task::spawn_blocking(move || {
            use rabex_env::resolver::EnvResolver;

            let version = env.unity_version().map(ToString::to_string)?;
            let bundle_version = env
                .load_serialized("globalgamemanagers")
                .and_then(|ggm| ggm.find_object_of::<PlayerSettingsSlim>())
                .ok()
                .flatten()
                .and_then(|ps| ps.bundleVersion);
            // `bundleVersion` stays "1.0" (or empty) for many older Unity
            // games that never bumped it — fall back to the game's own
            // `Constants.GAME_VERSION` constant in Assembly-CSharp.dll.
            let bundle_version = match bundle_version.as_deref() {
                Some(v) if v != "1.0" => bundle_version,
                _ => env
                    .game_files
                    .read_path(std::path::Path::new("Managed/Assembly-CSharp.dll"))
                    .ok()
                    .and_then(|bytes| {
                        transform::unity::game_version::extract_game_version(bytes.as_ref())
                    })
                    .or(bundle_version),
            };
            Ok::<_, anyhow::Error>((version, bundle_version))
        })
        .await
        .map_err(|e| ApiError::internal(format!("game-info task panicked: {e}")))?
        .map_err(|e| ApiError::internal(e.to_string()))?;

        Ok(GameInfo {
            engine: Some(EngineInfo::Unity {
                version,
                bundle_version,
            }),
        })
    }
}

#[cfg(not(feature = "unity"))]
async fn detect_engine(
    _state: &AppState,
    _appid: AppId,
    _depot_id: DepotId,
    _manifest_id: ManifestId,
    _branch: &str,
    _snapshot: Arc<Snapshot>,
) -> Result<GameInfo> {
    Ok(GameInfo { engine: None })
}
