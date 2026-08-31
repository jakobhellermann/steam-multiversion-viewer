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

use crate::config::{Config, VibrancyEffect};
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
    #[schema(value_type = String)]
    pub export_dir: Utf8PathBuf,
    pub vibrancy_effect: VibrancyEffect,
    pub vibrancy_tint: u8,
    pub restart_required: bool,
}

/// Partial config update — only fields the client wants to change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchConfig {
    #[schema(value_type = String)]
    pub store_root: Option<Utf8PathBuf>,
    #[schema(value_type = String)]
    pub mountpoint: Option<Utf8PathBuf>,
    #[schema(value_type = String)]
    pub export_dir: Option<Utf8PathBuf>,
    pub vibrancy_effect: Option<VibrancyEffect>,
    pub vibrancy_tint: Option<u8>,
}

fn build_config_dto(state: &AppState, saved: Config) -> ConfigDto {
    ConfigDto {
        // Only `store_root` requires a restart — it's baked into
        // `DepotStore` at init. `mountpoint` is live-reloaded on the
        // next `/api/mount/start`.
        restart_required: saved.store_root != state.initial_config.store_root,
        store_root: saved.store_root,
        mountpoint: saved.mountpoint,
        export_dir: saved.export_dir,
        vibrancy_effect: saved.vibrancy_effect,
        vibrancy_tint: saved.vibrancy_tint,
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
    // TODO: why not read from state?
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
    if let Some(export_dir) = body.export_dir {
        cfg.export_dir = export_dir;
    }
    if let Some(vibrancy_effect) = body.vibrancy_effect {
        cfg.vibrancy_effect = vibrancy_effect;
    }
    if let Some(vibrancy_tint) = body.vibrancy_tint {
        cfg.vibrancy_tint = vibrancy_tint.min(100);
    }
    cfg.save()?;
    // Publish the new config to every other request handler atomically.
    state.config.store(Arc::new(cfg.clone()));
    Ok(Json(build_config_dto(&state, cfg)))
}
