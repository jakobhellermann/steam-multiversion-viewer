use std::sync::{Arc, RwLock};
use std::time::Instant;

use anyhow::{Context, Result};
use steam_depot_vfs::chunk_store::{CdnChunkStore, FsCacheStore};
use steam_depot_vfs::fs::DepotSnapshot;
use steam_depot_vfs::{DepotStore, VfsError};

use crate::config::Config;
use crate::downloads::DownloadManager;
use crate::steam::{AppId, DepotId, ManifestId, SteamClient, auth};
use crate::store_index::StoreIndex;

pub type Snapshot = DepotSnapshot<FsCacheStore<CdnChunkStore<SteamClient>>>;

#[derive(Clone)]
pub struct AppState {
    pub steam: Arc<SteamClient>,
    pub store: Arc<DepotStore>,
    /// Snapshot of the config the running process started with.
    pub config: Arc<Config>,
    /// In-memory indexes derived from the on-disk store. Updated when new
    /// manifests are fetched.
    pub store_index: Arc<RwLock<StoreIndex>>,
    pub downloads: Arc<DownloadManager>,
}

impl AppState {
    pub async fn init() -> Result<Self> {
        let account = std::env::var("STEAM_USERNAME").context("STEAM_USERNAME not set")?;
        let password = std::env::var("STEAM_PASSWORD").context("STEAM_PASSWORD not set")?;

        let connection = auth::login(&account, &password).await?;
        let steam = Arc::new(SteamClient::new(connection));

        let config = Config::load_or_default()?;
        let store_root = config.store_root.clone();
        std::fs::create_dir_all(&store_root)
            .with_context(|| format!("creating store root {store_root}"))?;
        tracing::info!(root = %store_root, "depot store ready");
        let store = Arc::new(DepotStore::new(store_root.as_std_path().to_path_buf()));

        let index =
            StoreIndex::scan(&store).with_context(|| format!("scanning store {store_root}"))?;
        let store_index = Arc::new(RwLock::new(index));
        let downloads = DownloadManager::spawn(store_index.clone());

        Ok(Self {
            steam,
            store,
            config: Arc::new(config),
            store_index,
            downloads,
        })
    }

    /// Fetch (or load from cache) a manifest and fold it into the in-memory
    /// refcount index. All routes that need a manifest should go through
    /// this so `bytes_unique` stays consistent.
    pub async fn open_manifest(
        &self,
        app_id: AppId,
        depot_id: DepotId,
        manifest_gid: ManifestId,
        branch: &str,
    ) -> Result<Snapshot, VfsError> {
        let started = Instant::now();
        let snap = self
            .store
            .open_depot_manifest(
                self.steam.clone(),
                app_id.0,
                depot_id.0,
                manifest_gid.0,
                branch,
            )
            .await?;
        let fresh = self
            .store_index
            .write()
            .expect("store_index poisoned")
            .add_manifest(snap.manifest());
        tracing::info!(
            depot_id = %depot_id,
            gid = %manifest_gid,
            branch,
            cached = !fresh,
            time = ?started.elapsed(),
            "opened manifest"
        );
        Ok(snap)
    }
}
