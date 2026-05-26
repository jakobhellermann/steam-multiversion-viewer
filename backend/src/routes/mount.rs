// TODO(ai-review): review for style and correctness
//! `/api/mount/*` — toggle the FUSE filesystem and report its state.

use axum::Json;
use axum::extract::State;

use crate::error::ApiError;
use crate::mount::{MountControlError, MountStatus};
use crate::state::AppState;

#[utoipa::path(post, path = "/api/mount/start")]
#[tracing::instrument(skip_all)]
pub async fn start(State(state): State<AppState>) -> Result<Json<MountStatus>, ApiError> {
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
            state.steam.clone(),
            state.store.clone(),
            index_snapshot.into_iter(),
            &state.extra_manifests,
        )
        .map_err(mount_err)?;
    Ok(Json(status))
}

#[utoipa::path(post, path = "/api/mount/stop")]
#[tracing::instrument(skip_all)]
pub async fn stop(State(state): State<AppState>) -> Result<Json<MountStatus>, ApiError> {
    state.mount.stop().map_err(mount_err)?;
    Ok(Json(state.mount.status()))
}

#[utoipa::path(get, path = "/api/mount/status")]
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
    // The catch-all `impl From<E> for ApiError` would log, but we go
    // through `ApiError::new` so we can pick a specific status — emit
    // the same trace by hand so failures aren't silent.
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
