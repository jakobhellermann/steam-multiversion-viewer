//! History endpoints: files and structured nodes across a version set,
//! and a depot's manifest-level version history.

use axum::Json;
use axum::extract::{Path, State};

use crate::history::{
    FileHistoryEntry, FileHistoryRequest, ManifestHistoryEntry, ManifestHistoryRequest,
    build_file_history, build_manifest_history,
};
use crate::state::AppState;
use crate::steam::AppId;

use super::Result;

/// File history
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/file/history",
    tag = "history",
    request_body = FileHistoryRequest,
    responses((status = 200, body = Vec<FileHistoryEntry>))
)]
pub async fn file_history(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(request): Json<FileHistoryRequest>,
) -> Result<Json<Vec<FileHistoryEntry>>> {
    state.steam()?;
    Ok(Json(build_file_history(&state, appid, &request).await?))
}

/// Manifest history
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/history",
    tag = "history",
    request_body = ManifestHistoryRequest,
    responses((status = 200, body = Vec<ManifestHistoryEntry>))
)]
pub async fn manifest_history(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(request): Json<ManifestHistoryRequest>,
) -> Result<Json<Vec<ManifestHistoryEntry>>> {
    state.steam()?;
    Ok(Json(build_manifest_history(&state, appid, &request).await?))
}
