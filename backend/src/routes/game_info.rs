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
    Unity { version: String },
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

        let version =
            tokio::task::spawn_blocking(move || env.unity_version().map(ToString::to_string))
                .await
                .map_err(|e| ApiError::internal(format!("game-info task panicked: {e}")))?
                .map_err(|e| ApiError::internal(e.to_string()))?;

        return Ok((
            ImmutableCache,
            Json(GameInfo {
                engine: Some(EngineInfo::Unity {
                    version: version.to_string(),
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
