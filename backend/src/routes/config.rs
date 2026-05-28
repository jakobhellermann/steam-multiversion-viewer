// TODO(ai-review): review for style and correctness
//! Persistent config get + patch.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::json;
use utoipa::ToSchema;

use crate::config::Config;
use crate::state::AppState;

use super::Result;

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "store_root": "/home/alice/.local/share/steam-multiversion-viewer/store",
    "mountpoint": "/home/alice/steam-vfs",
    "restart_required": false
}))]
pub struct ConfigDto {
    #[schema(value_type = String)]
    pub store_root: Utf8PathBuf,
    #[schema(value_type = String)]
    pub mountpoint: Utf8PathBuf,
    pub restart_required: bool,
}

/// Partial config update — only fields the client wants to change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchConfig {
    #[schema(value_type = String)]
    pub store_root: Option<Utf8PathBuf>,
    #[schema(value_type = String)]
    pub mountpoint: Option<Utf8PathBuf>,
}

fn build_config_dto(state: &AppState, saved: Config) -> ConfigDto {
    ConfigDto {
        // Only `store_root` requires a restart — it's baked into
        // `DepotStore` at init. `mountpoint` is live-reloaded on the
        // next `/api/mount/start`.
        restart_required: saved.store_root != state.initial_config.store_root,
        store_root: saved.store_root,
        mountpoint: saved.mountpoint,
    }
}

/// Get persisted config
#[utoipa::path(
    get,
    path = "/api/config",
    tag = "config",
    responses((status = 200, body = ConfigDto))
)]
pub async fn get_config(State(state): State<AppState>) -> Result<Json<ConfigDto>> {
    let saved = Config::load_or_default()?;
    Ok(Json(build_config_dto(&state, saved)))
}

/// Patch persisted config
#[utoipa::path(
    patch,
    path = "/api/config",
    request_body = PatchConfig,
    tag = "config",
    responses((status = 200, body = ConfigDto))
)]
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
    if let Some(mountpoint) = body.mountpoint {
        // Only validated when the user actually starts the mount; we
        // intentionally don't create the dir here.
        cfg.mountpoint = mountpoint;
    }
    cfg.save()?;
    // Publish the new config to every other request handler atomically.
    state.config.store(Arc::new(cfg.clone()));
    Ok(Json(build_config_dto(&state, cfg)))
}
