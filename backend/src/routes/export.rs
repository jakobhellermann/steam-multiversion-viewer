//! `/api/.../export` — materialize a manifest into a directory under the configured export root.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::http::ApiError;
use crate::state::AppState;
use crate::state::export::{ExportError, ExportStatus, ExportTarget, check_single_component};
use crate::steam::{AppId, DepotId, ManifestId};

use super::{Result, default_branch};

#[derive(Debug, Deserialize, ToSchema)]
pub struct ExportManifestBody {
    #[serde(default = "default_branch")]
    pub branch: String,
    /// Single directory name below the configured export root.
    #[serde(default)]
    pub subdir: Option<String>,
}

/// Export a manifest to disk
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/export",
    tag = "export",
    request_body = ExportManifestBody,
    responses((status = 200, body = ExportStatus))
)]
#[tracing::instrument(skip_all)]
pub async fn manifest_export(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Json(body): Json<ExportManifestBody>,
) -> Result<Json<ExportStatus>> {
    state.steam()?; // 401 if not logged in

    let subdir = match body.subdir {
        Some(subdir) => check_single_component(&subdir)
            .map_err(export_err)?
            .to_owned(),
        None => format!("{appid}-{depot_id}-{manifest_id}"),
    };
    let target_dir = state.config.load().export_dir.join(subdir);

    let snapshot = state
        .open_manifest(appid, depot_id, manifest_id, &body.branch)
        .await?;
    // Steam's manifests rarely carry the executable flag; the app's launch
    // config is where the runnable targets are actually named.
    let launch_targets: Vec<String> = state
        .steam()?
        .depot
        .app_info(appid.0)
        .await?
        .config
        .launch
        .values()
        .map(|l| l.executable.clone())
        .collect();

    let target = ExportTarget {
        app_id: appid,
        depot_id,
        manifest_id,
    };
    let status = state
        .exports
        .start(
            Arc::new(snapshot),
            target,
            target_dir.clone(),
            launch_targets,
        )
        .map_err(export_err)?;
    tracing::info!(
        %depot_id, %manifest_id, branch = %body.branch, %target_dir,
        files = status.files_total,
        bytes = status.bytes_total,
        "export started"
    );
    Ok(Json(status))
}

/// Current export progress
#[utoipa::path(
    get,
    path = "/api/export",
    tag = "export",
    responses((status = 200, body = ExportStatus))
)]
pub async fn export_status(State(state): State<AppState>) -> Json<ExportStatus> {
    Json(state.exports.current())
}

/// Cancel the running export
#[utoipa::path(
    post,
    path = "/api/export/cancel",
    tag = "export",
    responses((status = 200, body = ExportStatus))
)]
#[tracing::instrument(skip_all)]
pub async fn export_cancel(State(state): State<AppState>) -> Json<ExportStatus> {
    state.exports.cancel();
    Json(state.exports.current())
}

fn export_err(e: ExportError) -> ApiError {
    let status = match &e {
        ExportError::AlreadyRunning => StatusCode::CONFLICT,
        ExportError::UnsafePath(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ExportError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError::new(status, e.to_string())
}
