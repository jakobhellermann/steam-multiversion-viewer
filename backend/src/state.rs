use std::sync::Arc;

use anyhow::{Context, Result};
use steam_depot_vfs::DepotStore;

use crate::config::Config;
use crate::steam::{SteamClient, auth};

#[derive(Clone)]
pub struct AppState {
    pub steam: Arc<SteamClient>,
    pub store: Arc<DepotStore>,
    pub config: Arc<Config>,
}

impl AppState {
    pub async fn init() -> Result<Self> {
        let account = std::env::var("STEAM_USERNAME").context("STEAM_USERNAME not set")?;
        let password = std::env::var("STEAM_PASSWORD").context("STEAM_PASSWORD not set")?;

        let connection = auth::login(&account, &password).await?;
        let steam = Arc::new(SteamClient::new(connection));

        let config = Config::load_or_default()?;
        let store_root = &config.store_root;
        std::fs::create_dir_all(store_root)
            .with_context(|| format!("creating store root {store_root}"))?;
        tracing::info!(root = %store_root, "depot store ready");
        let store = Arc::new(DepotStore::new(store_root.as_std_path().to_path_buf()));

        tracing::info!(steam_id = %steam.connection.steam_id().steam3(), "logged in");
        Ok(Self {
            steam,
            store,
            config: Arc::new(config),
        })
    }
}
