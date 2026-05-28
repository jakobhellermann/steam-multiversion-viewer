pub mod downloads;
pub mod extra_manifests;
pub mod manifest_cache;
pub mod mount;
pub mod store_index;

use std::sync::{Arc, RwLock};
use std::time::Instant;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use steam_depot_vfs::chunk_store::{CdnChunkStore, FsCacheStore};
use steam_depot_vfs::fs::DepotManifestStore;
use steam_depot_vfs::{DepotStore, VfsError};

use self::downloads::DownloadManager;
use self::extra_manifests::ExtraManifestsStore;
use self::mount::MountManager;
use self::store_index::StoreIndex;
use crate::config::Config;
use crate::steam::chunk_store::TrackedChunkStore;
use crate::steam::{AppId, DepotId, ManifestId, SteamClient, auth};

/// Concrete chunk-store stack used inside every opened manifest. Exposed
/// as an alias so per-manifest caches (e.g. the unity `Environment`)
/// can name the resolver type without re-typing the wrapping chain.
pub type SnapChunkStore = FsCacheStore<TrackedChunkStore<CdnChunkStore<SteamClient>>>;
pub type Snapshot = DepotManifestStore<SnapChunkStore>;

#[derive(Clone)]
pub struct AppState {
    pub steam: Arc<SteamClient>,
    pub store: Arc<DepotStore>,
    /// Latest config. Updated atomically by `PATCH /api/config`; read
    /// via `state.config.load()` to get an `Arc<Config>` snapshot that
    /// the caller can hold across awaits without locking.
    pub config: Arc<ArcSwap<Config>>,
    /// Snapshot of the config as it was when this process started. Used
    /// to surface `restart_required` for fields that aren't live-reloadable
    /// (e.g. `store_root`, which is baked into `DepotStore`).
    pub initial_config: Arc<Config>,
    /// In-memory indexes derived from the on-disk store. Updated when new
    /// manifests are fetched.
    pub store_index: Arc<RwLock<StoreIndex>>,
    pub downloads: Arc<DownloadManager>,
    pub extra_manifests: Arc<ExtraManifestsStore>,
    pub mount: Arc<MountManager>,
    pub manifest_cache: Arc<manifest_cache::ManifestCache>,
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

        let extra_manifests = Arc::new(
            ExtraManifestsStore::load(&store_root)
                .with_context(|| format!("loading extra manifests from {store_root}"))?,
        );

        let mount = Arc::new(MountManager::new());

        Ok(Self {
            steam,
            store,
            initial_config: Arc::new(config.clone()),
            config: Arc::new(ArcSwap::from_pointee(config)),
            store_index,
            downloads,
            extra_manifests,
            mount,
            manifest_cache: Arc::new(manifest_cache::ManifestCache::new()),
        })
    }

    /// Fetch (or load from cache) a manifest and fold it into the in-memory
    /// refcount index. All routes that need a manifest should go through
    /// this so `bytes_unique` stays consistent.
    pub async fn open_manifest(
        &self,
        app_id: AppId,
        depot_id: DepotId,
        manifest_id: ManifestId,
        branch: &str,
    ) -> Result<Snapshot, VfsError> {
        let started = Instant::now();
        let downloads = self.downloads.clone();
        let snap = self
            .store
            .open_depot_manifest_with_chunks(
                self.steam.clone(),
                app_id.0,
                depot_id.0,
                manifest_id.0,
                branch,
                move |cdn| TrackedChunkStore::new(cdn, downloads),
            )
            .await?;
        let fresh = self
            .store_index
            .write()
            .expect("store_index poisoned")
            .add_manifest(app_id, snap.manifest());
        tracing::trace!(
            depot_id = %depot_id,
            manifest_id = %manifest_id,
            branch,
            cached = !fresh,
            time = ?started.elapsed(),
            "opened manifest"
        );
        Ok(snap)
    }
}
