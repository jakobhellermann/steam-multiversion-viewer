use std::sync::Arc;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use steam_depot_vfs::DepotStore;

use crate::steam::{SteamClient, auth};

#[derive(Clone)]
pub struct AppState {
    pub steam: Arc<SteamClient>,
    pub store: Arc<DepotStore>,
}

impl AppState {
    pub async fn init() -> Result<Self> {
        let account = std::env::var("STEAM_USERNAME").context("STEAM_USERNAME not set")?;
        let password = std::env::var("STEAM_PASSWORD").context("STEAM_PASSWORD not set")?;

        let connection = auth::login(&account, &password).await?;
        let steam = Arc::new(SteamClient::new(connection));

        let dirs = ProjectDirs::from("", "", "steam-multiversion-viewer")
            .context("user data dir not supported on this platform")?;
        let store_root = dirs.data_dir().join("store");
        std::fs::create_dir_all(&store_root)
            .with_context(|| format!("creating store root {}", store_root.display()))?;
        tracing::info!(root = %store_root.display(), "depot store ready");
        let store = Arc::new(DepotStore::new(store_root));

        tracing::info!(steam_id = %steam.connection.steam_id().steam3(), "logged in");
        Ok(Self { steam, store })
    }
}
