// TODO(ai-review): review for style and correctness
//! `/api/mount/*` — toggle the depot filesystem and report its state.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;

use crate::http::ApiError;
use crate::state::AppState;
use crate::state::mount::{MountControlError, MountStatus};

/// Start depot mount
#[utoipa::path(
    post,
    path = "/api/mount/start",
    tag = "mount",
    responses((status = 200, body = MountStatus))
)]
#[tracing::instrument(skip_all)]
pub async fn start(State(state): State<AppState>) -> Result<Json<MountStatus>, ApiError> {
    let steam = state.steam()?; // 401 if not logged in
    let cfg = state.config.load();
    // Snapshot the index under its lock so we can drop it before
    // entering Mount::start (which acquires the mount's own lock).
    let index_snapshot = state
        .store_index
        .read()
        .expect("store_index poisoned")
        .indexed()
        .collect::<Vec<_>>();
    let mountpoint = cfg.mountpoint.as_std_path().to_path_buf();
    let status = state
        .mount
        .start_with(
            mountpoint,
            tokio::runtime::Handle::current(),
            steam,
            Arc::clone(&state.store),
            index_snapshot.into_iter(),
            &state.extra_manifests,
        )
        .await
        .map_err(mount_err)?;
    Ok(Json(status))
}

/// Stop depot mount
#[utoipa::path(
    post,
    path = "/api/mount/stop",
    tag = "mount",
    responses((status = 200, body = MountStatus))
)]
#[tracing::instrument(skip_all)]
pub async fn stop(State(state): State<AppState>) -> Result<Json<MountStatus>, ApiError> {
    state.mount.stop().await.map_err(mount_err)?;
    Ok(Json(state.mount.status()))
}

/// Depot mount status
#[utoipa::path(
    get,
    path = "/api/mount/status",
    tag = "mount",
    responses((status = 200, body = MountStatus))
)]
#[tracing::instrument(skip_all)]
pub async fn status(State(state): State<AppState>) -> Json<MountStatus> {
    Json(state.mount.status())
}

fn mount_err(e: MountControlError) -> ApiError {
    use axum::http::StatusCode;
    let status = match &e {
        MountControlError::AlreadyMounted | MountControlError::NotMounted => StatusCode::CONFLICT,
        MountControlError::Unsupported => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    tracing::error!(error = %e, source = ?source_chain(&e), "mount control failed");
    ApiError::new(status, e.to_string())
}

fn source_chain(err: &dyn std::error::Error) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = err.source();
    while let Some(e) = cur {
        out.push(e.to_string());
        cur = e.source();
    }
    out
}
