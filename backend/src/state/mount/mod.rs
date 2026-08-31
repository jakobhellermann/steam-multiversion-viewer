// TODO(ai-review): review for style and correctness
//! Mount manager.
//!
//! Owns a single optional mount that the user toggles via the
//! `/api/mount/*` routes. When started, every manifest currently in the
//! store index (plus every entry in `extra_manifests.json`) is
//! registered lazily so the depot files appear at
//! `<mountpoint>/<app_id>/<depot_id>/<manifest_gid>/…` without
//! pre-fetching anything — the first filesystem op on a manifest
//! triggers the actual manifest open.
//!
//! The active mount holds onto an [`Arc<SteamClient>`] and
//! [`Arc<DepotStore>`] cloned out of `AppState`, so it survives
//! independently of any single request.
//!
//! Back ends per platform: FUSE on linux, a loopback NFSv3 server on
//! macOS (no macFUSE, no kernel extension, no elevated privileges), and
//! ProjFS on Windows. The platform specifics live in [`platform`]; this
//! module is back-end-neutral.

use std::path::PathBuf;
use std::sync::Arc;

#[allow(unused_imports)]
use serde_json::json;

use parking_lot::Mutex;
use steam_depot_vfs::DepotStore;
use tokio::runtime::Handle;

use super::downloads::ChunkService;
use super::extra_manifests::ExtraManifestsStore;
use crate::steam::chunk_store::TrackedChunkStore;
use crate::steam::{AppId, DepotId, ManifestId, SteamClient};

mod platform;
pub use platform::prompt_enable_projfs;
use platform::{Backend, start_backend, unmount_backend};

pub struct MountManager {
    slot: Mutex<Option<Slot>>,
}

/// Held busy for the whole of both start and stop, so two starts can't
/// both mount and a start can't reclaim the mountpoint mid-unmount.
enum Slot {
    Starting,
    Mounted(Active),
    Stopping,
}

struct Active {
    mount: Arc<Backend>,
    mountpoint: PathBuf,
}

/// What each lazy mount entry needs to open its manifest on first access.
#[derive(Clone)]
pub struct MountDeps {
    pub steam: Arc<SteamClient>,
    pub store: Arc<DepotStore>,
    pub downloads: Arc<ChunkService>,
}

impl MountManager {
    pub fn new() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }

    /// Claim the single mount slot for a start that is about to run.
    fn reserve(&self) -> Result<(), MountControlError> {
        let mut slot = self.slot.lock();
        if slot.is_some() {
            return Err(MountControlError::AlreadyMounted);
        }
        *slot = Some(Slot::Starting);
        Ok(())
    }

    /// Give the slot back after a start that didn't get to mount.
    fn release(&self) {
        *self.slot.lock() = None;
    }

    pub fn status(&self) -> MountStatus {
        // A start or stop in flight reads as idle: it is a caller's own
        // request that is still running, and it will report the final
        // state when it returns.
        match &*self.slot.lock() {
            Some(Slot::Mounted(active)) => MountStatus::Mounted {
                mountpoint: active.mountpoint.clone(),
            },
            Some(Slot::Starting | Slot::Stopping) | None => MountStatus::Idle,
        }
    }

    /// Mount at `mountpoint` and register every entry in `index_snapshot`
    /// plus every row in `extra_manifests` as a lazy entry. The snapshot is
    /// taken by the caller (typically
    /// `store_index.read().iter_indexed().collect()`) so we don't hold the
    /// read lock across the mount start.
    pub async fn start_with(
        &self,
        mountpoint: PathBuf,
        rt: Handle,
        deps: MountDeps,
        index_snapshot: impl IntoIterator<Item = (AppId, DepotId, ManifestId)>,
        extra_manifests: &ExtraManifestsStore,
    ) -> Result<MountStatus, MountControlError> {
        self.reserve()?;
        let mount = match start_backend(&mountpoint, rt).await {
            Ok(mount) => Arc::new(mount),
            Err(e) => {
                self.release();
                return Err(e);
            }
        };

        // Dedup across store-index entries and extra-manifests entries;
        // either source can mention the same `(app, depot, gid)` and
        // `add_lazy` would return `AlreadyMounted` for the second insert.
        let mut registered = 0usize;
        let mut seen = std::collections::HashSet::new();
        for (app_id, depot_id, manifest_id) in index_snapshot {
            if !seen.insert((app_id, depot_id, manifest_id)) {
                continue;
            }
            register_one(&mount, &deps, app_id, depot_id, manifest_id, "public");
            registered += 1;
        }
        for (app_id, entries) in extra_manifests.get_all() {
            for e in entries {
                if !seen.insert((app_id, e.depot_id, e.manifest_id)) {
                    continue;
                }
                let branch = e.branch.as_deref().unwrap_or("public").to_string();
                register_one_with_branch(&mount, &deps, app_id, e.depot_id, e.manifest_id, branch);
                registered += 1;
            }
        }

        tracing::info!(
            mountpoint = %mountpoint.display(),
            registered,
            "mount started",
        );
        *self.slot.lock() = Some(Slot::Mounted(Active {
            mount: Arc::clone(&mount),
            mountpoint: mountpoint.clone(),
        }));
        Ok(MountStatus::Mounted { mountpoint })
    }

    /// Best-effort unmount for process teardown. A mount left behind
    /// hangs every access to it until something forces it out, so this
    /// runs on the way out and only logs what it can't fix.
    pub async fn stop_on_shutdown(&self) {
        match self.stop().await {
            Ok(()) | Err(MountControlError::NotMounted | MountControlError::Unsupported) => {}
            Err(e) => tracing::error!(%e, "unmounting on shutdown failed"),
        }
    }

    /// Unmount and drop the mount session. Returns `NotMounted` if
    /// nothing was mounted.
    pub async fn stop(&self) -> Result<(), MountControlError> {
        // Take the mount out but mark the slot `Stopping`, so a concurrent
        // start can't reclaim the mountpoint mid-unmount. Anything but a
        // live mount: nothing here to stop.
        let active = {
            let mut slot = self.slot.lock();
            match slot.take() {
                Some(Slot::Mounted(active)) => {
                    *slot = Some(Slot::Stopping);
                    active
                }
                other => {
                    *slot = other;
                    return Err(MountControlError::NotMounted);
                }
            }
        };
        // Try to unwrap the Arc — if any callback still holds a clone we
        // can't run the unmount, so fall through to dropping the Arc and
        // let the backend tear down when the last ref goes. Either way the
        // mount is gone, so clear the slot before we surface any error.
        let res = match Arc::try_unwrap(active.mount) {
            Ok(mount) => unmount_backend(mount).await,
            Err(_) => {
                tracing::warn!(
                    "mount stop: outstanding Arc references; falling back to drop-on-last-ref",
                );
                Ok(())
            }
        };
        *self.slot.lock() = None;
        res
    }
}

fn register_one(
    mount: &Backend,
    deps: &MountDeps,
    app_id: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
) {
    register_one_with_branch(
        mount,
        deps,
        app_id,
        depot_id,
        manifest_id,
        branch.to_string(),
    )
}

fn register_one_with_branch(
    mount: &Backend,
    deps: &MountDeps,
    app_id: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: String,
) {
    let deps = deps.clone();
    let res = mount.add_lazy(
        app_id.0,
        depot_id.0,
        manifest_id.0,
        move || {
            let MountDeps {
                steam,
                store,
                downloads,
            } = deps.clone();
            let branch = branch.clone();
            async move {
                tracing::info!(
                    %app_id, %depot_id, %manifest_id, branch,
                    "opening manifest on first access",
                );
                store
                    .open_depot_manifest_with_chunks(
                        steam,
                        app_id.0,
                        depot_id.0,
                        manifest_id.0,
                        &branch,
                        move |cdn| TrackedChunkStore::new(cdn, downloads),
                    )
                    .await
                    .map_err(|e| std::io::Error::other(e.to_string()))
            }
        },
        None,
    );
    if let Err(e) = res {
        tracing::warn!(%app_id, %depot_id, %manifest_id, %e, "mount add_lazy failed");
    }
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
#[allow(dead_code)] // Unsupported is unreachable on supported platforms; kept for the wire shape.
#[schema(example = json!({"state": "idle"}))]
pub enum MountStatus {
    Idle,
    Mounted {
        #[schema(value_type = String)]
        mountpoint: PathBuf,
    },
    /// This build / OS doesn't support mounting.
    Unsupported,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // Some variants are backend-specific.
pub enum MountControlError {
    #[error("a mount is already active; stop it first")]
    AlreadyMounted,
    #[error("no mount is currently active")]
    NotMounted,
    #[error("could not prepare mountpoint: {0}")]
    PrepareMountpoint(#[source] std::io::Error),
    #[error("could not start the mount: {0}")]
    Start(#[source] std::io::Error),
    #[error("could not unmount: {0}")]
    Unmount(#[source] std::io::Error),
    #[error("mounting is not supported on this platform")]
    Unsupported,
    #[error("the Windows Projected File System feature is not enabled")]
    ProjFsNotEnabled,
    #[error("could not enable ProjFS: {0}")]
    EnableProjFs(#[source] std::io::Error),
}

/// Result of prompting the user to enable the ProjFS feature (Windows).
// only constructed on Windows; type stays compiled for the cross-platform API schema
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjfsEnableOutcome {
    Enabled,
    RestartNeeded,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_in_flight_blocks_a_second_one() {
        let manager = MountManager::new();
        manager.reserve().expect("first start claims the slot");
        assert!(
            matches!(manager.reserve(), Err(MountControlError::AlreadyMounted)),
            "a second start must be rejected while the first is still mounting",
        );
    }

    #[tokio::test]
    async fn stopping_leaves_a_start_in_flight_alone() {
        let manager = MountManager::new();
        manager.reserve().expect("claim");
        assert!(
            matches!(manager.stop().await, Err(MountControlError::NotMounted)),
            "there is nothing mounted yet to stop",
        );
        assert!(
            matches!(manager.reserve(), Err(MountControlError::AlreadyMounted)),
            "and the in-flight start keeps its claim",
        );
    }

    #[tokio::test]
    async fn shutdown_teardown_tolerates_having_nothing_to_unmount() {
        let manager = MountManager::new();
        manager.stop_on_shutdown().await;
        manager
            .reserve()
            .expect("an untouched slot is still free afterwards");
    }

    #[test]
    fn a_start_that_failed_frees_the_slot_again() {
        let manager = MountManager::new();
        manager.reserve().expect("claim");
        manager.release();
        manager
            .reserve()
            .expect("the slot is free after a failed start");
    }
}
