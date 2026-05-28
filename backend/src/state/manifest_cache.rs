// TODO(ai-review): review for style and correctness
//! Bounded LRU of per-manifest scratch space. The cache itself is
//! format-agnostic; each entry's [`ManifestScratch`] hosts the
//! lazy-initialised, manifest-specific state that callers want to
//! reuse across requests (e.g. a unity [`UnityScratch::env`]).
//!
//! Capacity is small (2) because realistic workloads juggle a left+
//! right pair (diff view) at most.

use std::collections::VecDeque;
#[cfg(feature = "unity")]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

use crate::steam::{AppId, DepotId, ManifestId};

#[cfg(feature = "unity")]
use {
    super::{SnapChunkStore, Snapshot},
    rabex_env::rabex::tpk::TpkTypeTreeBlob,
    rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache,
    rabex_env::resolver::EnvResolver,
    rabex_env_steam_depot_vfs::SteamDepotGameFiles,
};

const CAP: usize = 2;

type Key = (AppId, DepotId, ManifestId, String);

/// Project-wide rabex `Environment`, defaulted to our concrete resolver
/// + TPK shape. Override the params for tests or alternative stores.
#[cfg(feature = "unity")]
pub type Environment<R = SteamDepotGameFiles<SnapChunkStore>, P = TypeTreeCache<TpkTypeTreeBlob>> =
    rabex_env::Environment<R, P>;

#[derive(Default)]
pub struct ManifestScratch {
    /// Outer `OnceLock`: has the unity probe run? Inner `Option`: is it
    /// a unity game? Caching the negative avoids re-probing the data
    /// dir on every request to a non-unity manifest.
    #[cfg(feature = "unity")]
    unity: OnceLock<Option<UnityScratch>>,
}

#[cfg(feature = "unity")]
pub struct UnityScratch {
    /// Shared per-manifest rabex env. Owns its `TypeTreeCache` because
    /// typetree resolution is unity-version specific — sharing across
    /// versions would silently hand back wrong trees.
    pub env: Arc<Environment>,
    /// `Some(key)` if this game ships SecurePlayerPrefs and the AES
    /// key could be extracted from `Managed/Assembly-CSharp.dll`,
    /// `None` otherwise. Either negative path (no DLL, type missing,
    /// IL pattern doesn't match) is cached so a non-securecplayerprefs
    /// game doesn't reparse the DLL on every TextAsset preview.
    spp_key: OnceLock<Option<Vec<u8>>>,
}

#[cfg(feature = "unity")]
impl UnityScratch {
    /// Game's data dir as a path string (`"<Game>_Data"`), suitable for
    /// stripping from manifest-relative paths before handing them to
    /// `env.load_cached`, which works in data-dir-relative paths.
    pub fn data_dir(&self) -> String {
        self.env.game_files.data_dir().display().to_string()
    }

    /// Lazy-extract the SecurePlayerPrefs AES key from this manifest's
    /// `Managed/Assembly-CSharp.dll`. Blocking — call from inside a
    /// `spawn_blocking` (the unity dump paths already do). Result is
    /// cached for the lifetime of the scratch; `None` means "no
    /// SecurePlayerPrefs in this game" and the caller should fall back
    /// to the raw asset bytes.
    pub fn secure_player_prefs_key(&self) -> Option<&[u8]> {
        self.spp_key
            .get_or_init(|| {
                let bytes = self
                    .env
                    .game_files
                    .read_path(std::path::Path::new("Managed/Assembly-CSharp.dll"))
                    .ok()?;
                transform::unity::secure_player_prefs::extract_key(bytes.as_ref())
            })
            .as_deref()
    }
}

#[cfg(feature = "unity")]
impl ManifestScratch {
    /// Lazy-build the unity scratch for this manifest, or `None` if
    /// the manifest isn't a unity game (no `<game>_Data` dir).
    pub fn unity(&self, snapshot: Arc<Snapshot>) -> Option<&UnityScratch> {
        self.unity
            .get_or_init(|| {
                SteamDepotGameFiles::new(snapshot).ok().map(|gf| {
                    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
                    UnityScratch {
                        env: Arc::new(Environment::new(gf, tpk)),
                        spp_key: OnceLock::new(),
                    }
                })
            })
            .as_ref()
    }

    /// Borrow the unity scratch that a previous [`Self::unity`] call
    /// already initialised — without needing a fresh `Arc<Snapshot>`.
    /// Returns `None` if probe hasn't run, or ran and decided this is
    /// not a unity manifest. Used by blocking-task closures that
    /// pre-acquired the scratch on the async side.
    pub fn unity_already_initialized(&self) -> Option<&UnityScratch> {
        self.unity.get()?.as_ref()
    }
}

#[derive(Default)]
pub struct ManifestCache {
    entries: Mutex<VecDeque<(Key, Arc<ManifestScratch>)>>,
}

impl ManifestCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hit-or-create. On hit the entry is bumped to the most-recent
    /// slot. On miss a fresh scratch is inserted and the oldest entry
    /// is evicted if we'd exceed [`CAP`].
    pub fn scratch(
        &self,
        app_id: AppId,
        depot_id: DepotId,
        manifest_id: ManifestId,
        branch: &str,
    ) -> Arc<ManifestScratch> {
        let key = (app_id, depot_id, manifest_id, branch.to_owned());
        let mut entries = self.entries.lock().expect("manifest_cache poisoned");
        if let Some(pos) = entries.iter().position(|(k, _)| k == &key) {
            let entry = entries.remove(pos).expect("position in bounds");
            let scratch = entry.1.clone();
            entries.push_back(entry);
            return scratch;
        }
        let scratch = Arc::new(ManifestScratch::default());
        entries.push_back((key, scratch.clone()));
        if entries.len() > CAP {
            entries.pop_front();
        }
        scratch
    }
}
