// TODO(ai-review): review for style and correctness
//! FUSE mount manager.
//!
//! Owns a single optional [`Mount`] that the user toggles via the
//! `/api/mount/*` routes. When started, every manifest currently in the
//! store index (plus every entry in `extra_manifests.json`) is
//! registered with [`Mount::add_lazy`] so the depot files appear at
//! `<mountpoint>/<app_id>/<depot_id>/<manifest_gid>/…` without
//! pre-fetching anything — the first FUSE op on a manifest triggers the
//! actual `DepotStore::open_depot_manifest` call.
//!
//! The active mount holds onto an [`Arc<SteamClient>`] and
//! [`Arc<DepotStore>`] cloned out of `AppState`, so it survives
//! independently of any single request.
//!
//! FUSE is linux-only — on other platforms `MountManager` exists but
//! every operation returns [`MountControlError::Unsupported`] and
//! `status()` returns [`MountStatus::Unsupported`]. The HTTP routes are
//! always wired up so the frontend can talk to them unchanged.

use std::path::PathBuf;
use std::sync::Arc;

#[allow(unused_imports)]
use serde_json::json;

use steam_depot_vfs::DepotStore;
use tokio::runtime::Handle;

use crate::extra_manifests::ExtraManifestsStore;
use crate::steam::{AppId, DepotId, ManifestId, SteamClient};

#[cfg(target_os = "linux")]
use parking_lot::Mutex;
#[cfg(target_os = "linux")]
use steam_depot_mount::{Mount, MountConfig, MountError};
#[cfg(target_os = "linux")]
use steam_depot_vfs::chunk_store::{CdnChunkStore, FsCacheStore};

#[cfg(target_os = "linux")]
type ChunkStoreC = FsCacheStore<CdnChunkStore<SteamClient>>;

pub struct MountManager {
    #[cfg(target_os = "linux")]
    active: Mutex<Option<Active>>,
}

#[cfg(target_os = "linux")]
struct Active {
    mount: Arc<Mount<ChunkStoreC>>,
    mountpoint: PathBuf,
}

impl MountManager {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            active: Mutex::new(None),
        }
    }

    pub fn status(&self) -> MountStatus {
        #[cfg(target_os = "linux")]
        {
            match &*self.active.lock() {
                Some(a) => MountStatus::Mounted {
                    mountpoint: a.mountpoint.clone(),
                },
                None => MountStatus::Idle,
            }
        }
        #[cfg(not(target_os = "linux"))]
        MountStatus::Unsupported
    }

    /// Mount at `mountpoint` and register every entry in
    /// `index_snapshot` plus every row in `extra_manifests` as a lazy
    /// entry. The snapshot is taken by the caller (typically
    /// `store_index.read().iter_indexed().collect()`) so we don't hold
    /// the read lock across the FUSE start.
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
    pub fn start_with(
        &self,
        mountpoint: PathBuf,
        rt: Handle,
        steam: Arc<SteamClient>,
        store: Arc<DepotStore>,
        index_snapshot: impl IntoIterator<Item = (AppId, DepotId, ManifestId)>,
        extra_manifests: &ExtraManifestsStore,
    ) -> Result<MountStatus, MountControlError> {
        #[cfg(not(target_os = "linux"))]
        return Err(MountControlError::Unsupported);

        #[cfg(target_os = "linux")]
        {
            let mut slot = self.active.lock();
            if slot.is_some() {
                return Err(MountControlError::AlreadyMounted);
            }
            prepare_mountpoint(&mountpoint).map_err(MountControlError::PrepareMountpoint)?;

            let mount: Mount<ChunkStoreC> = match start_fuse(&mountpoint, rt.clone()) {
                Ok(m) => m,
                Err(e) if is_stale_mount(&e) => {
                    // `prepare_mountpoint`'s read_dir check can succeed
                    // (path lists fine) even when the kernel still has a
                    // half-dead FUSE entry for it. Recover and retry once.
                    tracing::warn!(
                        path = %mountpoint.display(),
                        "FUSE session start failed with ENOTCONN; running fusermount -uz and retrying",
                    );
                    run_fusermount(&mountpoint, &["-uz"])
                        .map_err(MountControlError::PrepareMountpoint)?;
                    std::fs::create_dir_all(&mountpoint)
                        .map_err(MountControlError::PrepareMountpoint)?;
                    start_fuse(&mountpoint, rt).map_err(MountControlError::Start)?
                }
                Err(e) => return Err(MountControlError::Start(e)),
            };
            let mount = Arc::new(mount);

            // Dedup across store-index entries and extra-manifests entries;
            // either source can mention the same `(app, depot, gid)` and
            // `Mount::add_lazy` would return `AlreadyMounted` for the second
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
            *slot = Some(Active {
                mount: Arc::clone(&mount),
                mountpoint: mountpoint.clone(),
            });
            Ok(MountStatus::Mounted { mountpoint })
        }
    }

    /// Unmount and drop the FUSE session. Returns `NotMounted` if
    /// nothing was mounted.
    pub fn stop(&self) -> Result<(), MountControlError> {
        #[cfg(not(target_os = "linux"))]
        return Err(MountControlError::Unsupported);

        #[cfg(target_os = "linux")]
        {
            let Some(active) = self.active.lock().take() else {
                return Err(MountControlError::NotMounted);
            };
            // Try to unwrap the Arc — if any callback still holds a clone
            // we can't run `umount_and_join`, so fall through to dropping
            // the Arc and let fuser tear down when the last ref goes.
            match Arc::try_unwrap(active.mount) {
                Ok(mount) => mount.unmount().map_err(MountControlError::Unmount)?,
                Err(_) => {
                    tracing::warn!(
                        "mount stop: outstanding Arc references; falling back to drop-on-last-ref",
                    );
                }
            }
            Ok(())
        }
    }
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

#[cfg(target_os = "linux")]
fn register_one(
    mount: &Mount<ChunkStoreC>,
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

#[cfg(target_os = "linux")]
fn register_one_with_branch(
    mount: &Mount<ChunkStoreC>,
    steam: &Arc<SteamClient>,
    store: &Arc<DepotStore>,
    app_id: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: String,
) {
    let steam = Arc::clone(steam);
    let store = Arc::clone(store);
    let res = mount.add_lazy(app_id.0, depot_id.0, manifest_id.0, move || {
        let steam = Arc::clone(&steam);
        let store = Arc::clone(&store);
        let branch = branch.clone();
        async move {
            tracing::info!(
                %app_id, %depot_id, %manifest_id, branch,
                "opening manifest on first FUSE access",
            );
            store
                .open_depot_manifest(steam, app_id.0, depot_id.0, manifest_id.0, &branch)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))
        }
    });
    if let Err(e) = res {
        tracing::warn!(%app_id, %depot_id, %manifest_id, %e, "mount add_lazy failed");
    }
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
#[allow(dead_code)] // Idle/Mounted only constructed on linux; kept for the wire shape.
#[schema(example = json!({"state": "idle"}))]
pub enum MountStatus {
    Idle,
    Mounted {
        #[schema(value_type = String)]
        mountpoint: PathBuf,
    },
    /// This build / OS doesn't support FUSE mounts.
    Unsupported,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // Linux-only variants stay for parity with the linux build.
pub enum MountControlError {
    #[error("a mount is already active; stop it first")]
    AlreadyMounted,
    #[error("no mount is currently active")]
    NotMounted,
    #[error("could not prepare mountpoint: {0}")]
    PrepareMountpoint(#[source] std::io::Error),
    #[error("could not start FUSE session: {0}")]
    Start(#[source] std::io::Error),
    #[error("could not unmount: {0}")]
    Unmount(#[source] std::io::Error),
    #[error("FUSE mount is not supported on this platform")]
    Unsupported,
}
