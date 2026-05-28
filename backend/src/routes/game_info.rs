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
use crate::http::ImmutableCache;
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

use super::Result;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct GameInfoQuery {
    #[serde(default = "crate::routes::default_branch")]
    pub branch: String,
}

#[derive(Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
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
) -> Result<(ImmutableCache, Json<GameInfo>)> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    #[cfg(feature = "unity")]
    {
        let scratch = state
            .manifest_cache
            .scratch(appid, depot_id, manifest_id, &q.branch);
        let Some(unity) = scratch.unity(snapshot) else {
            return Ok((ImmutableCache, Json(GameInfo { engine: None })));
        };
        let env = unity.env.clone();

        // Both fields come out of globalgamemanagers — combine the
        // reads in one blocking call so we don't pay the deserialise
        // setup twice. PlayerSettings is best-effort: a missing or
        // unparsable object should not turn the whole game-info into
        // a 500, it just means we have no bundleVersion to report.
        let (version, bundle_version) = tokio::task::spawn_blocking(move || {
            let version = env.unity_version().map(ToString::to_string)?;
            let bundle_version = env
                .load_serialized("globalgamemanagers")
                .and_then(|ggm| ggm.find_object_of::<PlayerSettingsSlim>())
                .ok()
                .flatten()
                .and_then(|ps| ps.bundleVersion);
            Ok::<_, anyhow::Error>((version, bundle_version))
        })
        .await
        .map_err(|e| ApiError::internal(format!("game-info task panicked: {e}")))?
        .map_err(|e| ApiError::internal(e.to_string()))?;

        return Ok((
            ImmutableCache,
            Json(GameInfo {
                engine: Some(EngineInfo::Unity {
                    version,
                    bundle_version,
                }),
            }),
        ));
    }

    #[cfg(not(feature = "unity"))]
    {
        let _ = snapshot;
        Ok((ImmutableCache, Json(GameInfo { engine: None })))
    }
}
