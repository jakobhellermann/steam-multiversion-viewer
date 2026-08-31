pub mod downloads;
pub mod export;
pub mod extra_manifests;
pub mod manifest_cache;
pub mod mount;
pub mod store_index;
pub mod store_model;

use std::sync::{Arc, RwLock};
use std::time::Instant;

use anyhow::{Context, Result};
use arc_swap::{ArcSwap, ArcSwapOption};
use steam_depot_vfs::chunk_store::{CdnChunkStore, FsCacheStore};
use steam_depot_vfs::fs::DepotManifestStore;
use steam_depot_vfs::{DepotStore, VfsError};

use self::downloads::ChunkService;
use self::extra_manifests::ExtraManifestsStore;
use self::mount::MountManager;
use self::store_index::StoreIndex;
use self::store_model::StoreModel;
use crate::config::Config;
use crate::http::ApiError;
use crate::steam::chunk_store::TrackedChunkStore;
use crate::steam::{AppId, DepotId, ManifestId, SteamClient, auth};

/// Concrete chunk-store stack used inside every opened manifest. Exposed
/// as an alias so per-manifest caches (e.g. the unity `Environment`)
/// can name the resolver type without re-typing the wrapping chain.
pub type SnapChunkStore = FsCacheStore<TrackedChunkStore<CdnChunkStore<SteamClient>>>;
pub type Snapshot = DepotManifestStore<SnapChunkStore>;

#[derive(Clone)]
pub struct AppState {
    /// Authenticated Steam connection, or `None` until the user logs in
    /// via `/api/auth/login`. Swapped atomically so login/logout take
    /// effect without restarting the server. Read it through
    /// [`AppState::steam`] in request handlers to get a `401` when absent.
    pub steam: Arc<ArcSwapOption<SteamClient>>,
    /// The (at most one) in-progress interactive web login, so the auth
    /// status endpoint can report whether a code is needed.
    pub pending_login: auth::PendingLoginSlot,
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
    /// Derived cache for the store-management routes: built lazily, dropped
    /// whole on change (never partially updated). See [`store_model`](Self::store_model).
    store_model: Arc<RwLock<Option<Arc<StoreModel>>>>,
    pub downloads: Arc<ChunkService>,
    pub exports: Arc<export::ExportManager>,
    pub extra_manifests: Arc<ExtraManifestsStore>,
    pub mount: Arc<MountManager>,
    pub manifest_cache: Arc<manifest_cache::ManifestCache>,
}

impl AppState {
    pub async fn init() -> Result<Self> {
        // The Steam connection is established lazily via `/api/auth/login`
        // (or an optional background env-var login, see `main`), so the
        // server can start without credentials.
        let config = Config::load_or_default()?;
        let store_root = config.store_root.clone();
        std::fs::create_dir_all(&store_root)
            .with_context(|| format!("creating store root {store_root}"))?;
        tracing::info!(root = %store_root, "depot store ready");
        let store = Arc::new(DepotStore::new(store_root.as_std_path().to_path_buf()));

        let index =
            StoreIndex::scan(&store).with_context(|| format!("scanning store {store_root}"))?;
        let store_index = Arc::new(RwLock::new(index));
        let downloads = ChunkService::spawn(store_index.clone());
        let exports = export::ExportManager::new(Arc::clone(&downloads));

        let extra_manifests = Arc::new(
            ExtraManifestsStore::load(&store_root)
                .with_context(|| format!("loading extra manifests from {store_root}"))?,
        );

        let mount = Arc::new(MountManager::new());

        Ok(Self {
            steam: Arc::new(ArcSwapOption::empty()),
            pending_login: Arc::new(std::sync::Mutex::new(None)),
            store,
            initial_config: Arc::new(config.clone()),
            config: Arc::new(ArcSwap::from_pointee(config)),
            store_index,
            store_model: Arc::new(RwLock::new(None)),
            downloads,
            exports,
            extra_manifests,
            mount,
            manifest_cache: Arc::new(manifest_cache::ManifestCache::new()),
        })
    }

    /// The current store model (manifest→chunk graph).
    pub fn store_model(&self) -> Result<Arc<StoreModel>, std::io::Error> {
        if let Some(model) = self
            .store_model
            .read()
            .expect("store_model poisoned")
            .as_ref()
        {
            return Ok(Arc::clone(model));
        }
        let mut slot = self.store_model.write().expect("store_model poisoned");
        if let Some(model) = slot.as_ref() {
            return Ok(Arc::clone(model));
        }
        let model = Arc::new(StoreModel::build(&self.store)?);
        *slot = Some(Arc::clone(&model));
        Ok(model)
    }

    /// Call whenever the set of cached manifests changes.
    pub fn invalidate_store_model(&self) {
        *self.store_model.write().expect("store_model poisoned") = None;
    }

    /// The authenticated Steam client, or a `401` error when the user
    /// hasn't logged in yet. Request handlers that touch Steam should go
    /// through this so the logged-out case is a clean `Unauthorized`.
    pub fn steam(&self) -> Result<Arc<SteamClient>, ApiError> {
        self.steam
            .load_full()
            .ok_or_else(|| ApiError::unauthorized("not logged in to Steam"))
    }

    /// Install a freshly authenticated connection (login).
    pub fn set_steam(&self, client: SteamClient) {
        self.steam.store(Some(Arc::new(client)));
    }

    /// Drop the current connection (logout). The cached refresh token is
    /// kept on disk so the next login can skip the password.
    pub fn clear_steam(&self) {
        self.steam.store(None);
    }

    /// Fetch (or load from cache) a manifest and fold it into the in-memory
    /// refcount index. All routes that need a manifest should go through
    /// this so `bytes_unique` stays consistent. Handlers should guard with
    /// [`AppState::steam`] for a clean `401`; the not-logged-in case here is
    /// a defensive fallback.
    pub async fn open_manifest(
        &self,
        app_id: AppId,
        depot_id: DepotId,
        manifest_id: ManifestId,
        branch: &str,
    ) -> Result<Snapshot, VfsError> {
        let started = Instant::now();
        let downloads = Arc::clone(&self.downloads);
        let steam = self
            .steam
            .load_full()
            .ok_or_else(|| VfsError::Other("not logged in to Steam".into()))?;
        let snap = self
            .store
            .open_depot_manifest_with_chunks(
                steam,
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
        if fresh {
            // A newly-cached manifest changes the manifest→chunk graph.
            self.invalidate_store_model();
        }
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
