// TODO(ai-review): review for style and correctness
//! Mount manager.
//!
//! Owns a single optional mount that the user toggles via the
//! `/api/mount/*` routes. When started, every manifest currently in the
//! store index (plus every entry in `extra_manifests.json`) is
//! registered lazily so the depot files appear at
//! `<mountpoint>/<app_id>/<depot_id>/<manifest_gid>/…` without
//! pre-fetching anything — the first filesystem op on a manifest
//! triggers the actual `DepotStore::open_depot_manifest` call.
//!
//! The active mount holds onto an [`Arc<SteamClient>`] and
//! [`Arc<DepotStore>`] cloned out of `AppState`, so it survives
//! independently of any single request.
//!
//! Back ends per platform: FUSE on linux, a loopback NFSv3 server on
//! macOS (no macFUSE, no kernel extension, no elevated privileges).
//! Elsewhere `MountManager` exists but every operation returns
//! [`MountControlError::Unsupported`] and `status()` returns
//! [`MountStatus::Unsupported`]. The HTTP routes are always wired up so
//! the frontend can talk to them unchanged.

use std::path::PathBuf;
use std::sync::Arc;

#[allow(unused_imports)]
use serde_json::json;

use steam_depot_vfs::DepotStore;
use tokio::runtime::Handle;

use super::extra_manifests::ExtraManifestsStore;
use crate::steam::{AppId, DepotId, ManifestId, SteamClient};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use parking_lot::Mutex;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use steam_depot_vfs::chunk_store::{CdnChunkStore, FsCacheStore};

#[cfg(target_os = "linux")]
use steam_depot_mount::{Mount, MountConfig, MountError};
#[cfg(target_os = "macos")]
use steam_depot_mount::{NfsMount, NfsMountConfig};

#[cfg(any(target_os = "linux", target_os = "macos"))]
type ChunkStoreC = FsCacheStore<CdnChunkStore<SteamClient>>;

#[cfg(target_os = "linux")]
type Backend = Mount<ChunkStoreC>;
#[cfg(target_os = "macos")]
type Backend = NfsMount<ChunkStoreC>;

pub struct MountManager {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: Mutex<Option<Slot>>,
}

/// Held busy for the whole of both start and stop, so two starts can't
/// both mount and a start can't reclaim the mountpoint mid-unmount.
#[cfg(any(target_os = "linux", target_os = "macos"))]
enum Slot {
    Starting,
    Mounted(Active),
    Stopping,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct Active {
    mount: Arc<Backend>,
    mountpoint: PathBuf,
}

impl MountManager {
    pub fn new() -> Self {
        Self {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Mutex::new(None),
        }
    }

    /// Claim the single mount slot for a start that is about to run.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn reserve(&self) -> Result<(), MountControlError> {
        let mut slot = self.slot.lock();
        if slot.is_some() {
            return Err(MountControlError::AlreadyMounted);
        }
        *slot = Some(Slot::Starting);
        Ok(())
    }

    /// Give the slot back after a start that didn't get to mount.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn release(&self) {
        *self.slot.lock() = None;
    }

    pub fn status(&self) -> MountStatus {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            // A start or stop in flight reads as idle: it is a caller's
            // own request that is still running, and it will report the
            // final state when it returns.
            match &*self.slot.lock() {
                Some(Slot::Mounted(active)) => MountStatus::Mounted {
                    mountpoint: active.mountpoint.clone(),
                },
                Some(Slot::Starting | Slot::Stopping) | None => MountStatus::Idle,
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        MountStatus::Unsupported
    }

    /// Mount at `mountpoint` and register every entry in
    /// `index_snapshot` plus every row in `extra_manifests` as a lazy
    /// entry. The snapshot is taken by the caller (typically
    /// `store_index.read().iter_indexed().collect()`) so we don't hold
    /// the read lock across the mount start.
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "macos")),
        allow(unused_variables)
    )]
    pub async fn start_with(
        &self,
        mountpoint: PathBuf,
        rt: Handle,
        steam: Arc<SteamClient>,
        store: Arc<DepotStore>,
        index_snapshot: impl IntoIterator<Item = (AppId, DepotId, ManifestId)>,
        extra_manifests: &ExtraManifestsStore,
    ) -> Result<MountStatus, MountControlError> {
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return Err(MountControlError::Unsupported);

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
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
            // `add_lazy` would return `AlreadyMounted` for the second
            // insert.
            let mut registered = 0usize;
            let mut seen = std::collections::HashSet::new();
            for (app_id, depot_id, manifest_id) in index_snapshot {
                if !seen.insert((app_id, depot_id, manifest_id)) {
                    continue;
                }
                register_one(
                    &mount,
                    &steam,
                    &store,
                    app_id,
                    depot_id,
                    manifest_id,
                    "public",
                );
                registered += 1;
            }
            for (app_id, entries) in extra_manifests.get_all() {
                for e in entries {
                    if !seen.insert((app_id, e.depot_id, e.manifest_id)) {
                        continue;
                    }
                    let branch = e.branch.as_deref().unwrap_or("public").to_string();
                    register_one_with_branch(
                        &mount,
                        &steam,
                        &store,
                        app_id,
                        e.depot_id,
                        e.manifest_id,
                        branch,
                    );
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
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return Err(MountControlError::Unsupported);

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            // Take the mount out but mark the slot `Stopping`, so a
            // concurrent start can't reclaim the mountpoint mid-unmount.
            // Anything but a live mount: nothing here to stop.
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
            // Try to unwrap the Arc — if any callback still holds a clone
            // we can't run the unmount, so fall through to dropping the
            // Arc and let the backend tear down when the last ref goes.
            // Either way the mount is gone, so clear the slot before we
            // surface any unmount error.
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
}

#[cfg(target_os = "linux")]
async fn start_backend(
    mountpoint: &std::path::Path,
    rt: Handle,
) -> Result<Backend, MountControlError> {
    prepare_mountpoint(mountpoint).map_err(MountControlError::PrepareMountpoint)?;
    match start_fuse(mountpoint, rt.clone()) {
        Ok(m) => Ok(m),
        Err(e) if is_stale_mount(&e) => {
            // `prepare_mountpoint`'s read_dir check can succeed (path
            // lists fine) even when the kernel still has a half-dead
            // FUSE entry for it. Recover and retry once.
            tracing::warn!(
                path = %mountpoint.display(),
                "FUSE session start failed with ENOTCONN; running fusermount -uz and retrying",
            );
            run_fusermount(mountpoint, &["-uz"]).map_err(MountControlError::PrepareMountpoint)?;
            std::fs::create_dir_all(mountpoint).map_err(MountControlError::PrepareMountpoint)?;
            start_fuse(mountpoint, rt).map_err(MountControlError::Start)
        }
        Err(e) => Err(MountControlError::Start(e)),
    }
}

/// The NFS back end creates the mountpoint itself and needs no runtime
/// handle — it is async all the way down.
#[cfg(target_os = "macos")]
async fn start_backend(
    mountpoint: &std::path::Path,
    _rt: Handle,
) -> Result<Backend, MountControlError> {
    NfsMount::start(NfsMountConfig::new(mountpoint.to_path_buf()))
        .await
        .map_err(|e| MountControlError::Start(std::io::Error::other(e.to_string())))
}

#[cfg(target_os = "linux")]
async fn unmount_backend(mount: Backend) -> Result<(), MountControlError> {
    mount.unmount().map_err(MountControlError::Unmount)
}

#[cfg(target_os = "macos")]
async fn unmount_backend(mount: Backend) -> Result<(), MountControlError> {
    mount
        .unmount()
        .await
        .map_err(|e| MountControlError::Unmount(std::io::Error::other(e.to_string())))
}

/// Ensure `path` exists as an empty directory ready to be FUSE-mounted.
///
/// Crash-recovery: if a previous FUSE mount died, the kernel still has
/// an entry but `read_dir` returns ENOTCONN ("Transport endpoint is not
/// connected"). Try `fusermount -u`, then `fusermount -uz` (lazy) as a
/// fallback before giving up.
#[cfg(target_os = "linux")]
fn prepare_mountpoint(path: &std::path::Path) -> Result<(), std::io::Error> {
    // Fast path: dir already exists and is healthy, or we can create it.
    if let Some(()) = try_use(path)? {
        return Ok(());
    }
    // Stale mount. Try a clean unmount; if read_dir still fails, fall
    // back to a lazy unmount.
    run_fusermount(path, &["-u"])?;
    if try_use(path)?.is_some() {
        return Ok(());
    }
    tracing::warn!(
        path = %path.display(),
        "`fusermount -u` returned ok but the mount is still stale; retrying with -uz",
    );
    run_fusermount(path, &["-uz"])?;
    if try_use(path)?.is_some() {
        return Ok(());
    }
    Err(std::io::Error::other(format!(
        "mountpoint {} is still ENOTCONN after fusermount -u and -uz",
        path.display(),
    )))
}

/// Try to use `path` as an empty mountpoint dir. Returns `Some(())` on
/// success, `None` if the path is a stale FUSE mount that needs
/// recovery, `Err` for any other I/O error.
#[cfg(target_os = "linux")]
fn try_use(path: &std::path::Path) -> Result<Option<()>, std::io::Error> {
    match std::fs::create_dir_all(path) {
        Ok(()) => return Ok(Some(())),
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        Err(_) => {}
    }
    match std::fs::read_dir(path) {
        Ok(_) => Ok(Some(())),
        // 107 = ENOTCONN on linux — the path entry exists but its FUSE
        // server is gone.
        Err(e) if e.raw_os_error() == Some(107) => {
            tracing::warn!(
                path = %path.display(),
                "mountpoint has a stale FUSE mount; attempting recovery",
            );
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Run `Mount::start` and flatten its single-variant error wrapper down
/// to `io::Error` so callers don't see "FUSE error: FUSE error: …".
#[cfg(target_os = "linux")]
fn start_fuse(
    mountpoint: &std::path::Path,
    rt: tokio::runtime::Handle,
) -> Result<Mount<ChunkStoreC>, std::io::Error> {
    Mount::start(MountConfig::new(mountpoint.to_path_buf()), rt).map_err(|e| match e {
        MountError::Fuse(io) => io,
    })
}

/// True if `e` indicates the mountpoint has a half-dead FUSE entry the
/// kernel still remembers (a `fusermount -uz` is the usual fix).
#[cfg(target_os = "linux")]
fn is_stale_mount(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(107)
}

#[cfg(target_os = "linux")]
fn run_fusermount(path: &std::path::Path, args: &[&str]) -> Result<(), std::io::Error> {
    let status = std::process::Command::new("fusermount")
        .args(args)
        .arg(path)
        .status()
        .map_err(|e| std::io::Error::other(format!("spawning fusermount {args:?}: {e}")))?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "fusermount {} {} exited with {status}",
            args.join(" "),
            path.display(),
        )));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn register_one(
    mount: &Backend,
    steam: &Arc<SteamClient>,
    store: &Arc<DepotStore>,
    app_id: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
) {
    register_one_with_branch(
        mount,
        steam,
        store,
        app_id,
        depot_id,
        manifest_id,
        branch.to_string(),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn register_one_with_branch(
    mount: &Backend,
    steam: &Arc<SteamClient>,
    store: &Arc<DepotStore>,
    app_id: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: String,
) {
    let steam = Arc::clone(steam);
    let store = Arc::clone(store);
    let res = mount.add_lazy(
        app_id.0,
        depot_id.0,
        manifest_id.0,
        move || {
            let steam = Arc::clone(&steam);
            let store = Arc::clone(&store);
            let branch = branch.clone();
            async move {
                tracing::info!(
                    %app_id, %depot_id, %manifest_id, branch,
                    "opening manifest on first access",
                );
                store
                    .open_depot_manifest(steam, app_id.0, depot_id.0, manifest_id.0, &branch)
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
#[allow(dead_code)] // Idle/Mounted only constructed where a backend exists; kept for the wire shape.
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
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
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
