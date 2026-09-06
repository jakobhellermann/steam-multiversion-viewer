//! File and structured-node history endpoint.

use axum::Json;
use axum::extract::{Path, State};

use crate::history::{FileHistoryEntry, FileHistoryRequest, build_file_history};
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
