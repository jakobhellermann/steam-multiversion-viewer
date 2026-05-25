// TODO(ai-review): review for style and correctness
//! User-tracked manifests that aren't part of the live Steam API response.
//!
//! Steam only returns the manifests currently pointed at by a branch.
//! Older manifests (visible on steamdb) need to be entered by hand. This
//! module owns the on-disk JSON file at `{store_root}/extra_manifests.json`
//! that backs the corresponding API surface.

use std::collections::HashMap;
use std::sync::Mutex;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::steam::{AppId, DepotId, ManifestId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtraManifestEntry {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    #[serde(default)]
    pub branch: Option<String>,
}

/// Top-level on-disk format. Keyed by app id (as string for JSON friendliness).
#[derive(Debug, Default, Serialize, Deserialize)]
struct FileFormat {
    #[serde(default)]
    apps: HashMap<u32, Vec<ExtraManifestEntry>>,
}

pub struct ExtraManifestsStore {
    path: Utf8PathBuf,
    // Single-process app, so a plain mutex is fine — no contention worth
    // worrying about.
    state: Mutex<FileFormat>,
}

impl ExtraManifestsStore {
    pub fn load(store_root: &Utf8Path) -> Result<Self, std::io::Error> {
        let path = store_root.join("extra_manifests.json");
        let state = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            serde_json::from_str(&raw).map_err(std::io::Error::other)?
        } else {
            FileFormat::default()
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    pub fn list(&self, app_id: AppId) -> Vec<ExtraManifestEntry> {
        let state = self.state.lock().expect("extra_manifests poisoned");
        state.apps.get(&app_id.0).cloned().unwrap_or_default()
    }

    /// Snapshot of every app's manifest list. Used by the mount
    /// bootstrap to register everything in one go.
    pub fn get_all(&self) -> Vec<(AppId, Vec<ExtraManifestEntry>)> {
        let state = self.state.lock().expect("extra_manifests poisoned");
        state
            .apps
            .iter()
            .map(|(app, entries)| (AppId(*app), entries.clone()))
            .collect()
    }

    /// Replace the entire list for `app_id`. Dedups by (depot_id, manifest_id)
    /// preserving the first occurrence's branch.
    pub fn set(
        &self,
        app_id: AppId,
        entries: Vec<ExtraManifestEntry>,
    ) -> Result<Vec<ExtraManifestEntry>, std::io::Error> {
        let mut state = self.state.lock().expect("extra_manifests poisoned");
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<ExtraManifestEntry> = entries
            .into_iter()
            .filter(|e| seen.insert((e.depot_id, e.manifest_id)))
            .collect();
        if deduped.is_empty() {
            state.apps.remove(&app_id.0);
        } else {
            state.apps.insert(app_id.0, deduped.clone());
        }
        write_atomic(&self.path, &state)?;
        Ok(deduped)
    }

    /// Remove a single entry. Returns the new list.
    pub fn remove(
        &self,
        app_id: AppId,
        depot_id: DepotId,
        manifest_id: ManifestId,
    ) -> Result<Vec<ExtraManifestEntry>, std::io::Error> {
        let mut state = self.state.lock().expect("extra_manifests poisoned");
        let mut new_list = Vec::new();
        if let Some(list) = state.apps.get(&app_id.0) {
            new_list = list
                .iter()
                .filter(|e| !(e.depot_id == depot_id && e.manifest_id == manifest_id))
                .cloned()
                .collect();
        }
        if new_list.is_empty() {
            state.apps.remove(&app_id.0);
        } else {
            state.apps.insert(app_id.0, new_list.clone());
        }
        write_atomic(&self.path, &state)?;
        Ok(new_list)
    }
}

fn write_atomic(path: &Utf8Path, state: &FileFormat) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let raw = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, raw)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
