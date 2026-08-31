//! Platform-specific mount back ends behind a uniform interface, so the
//! [`super`] manager stays free of `cfg`. Each supported OS provides
//! `Backend`, `start_backend`, `unmount_backend`, and `prompt_enable_projfs`.

use tokio::runtime::Handle;

use steam_depot_vfs::chunk_store::{CdnChunkStore, FsCacheStore};

use crate::steam::SteamClient;
use crate::steam::chunk_store::TrackedChunkStore;

use super::{MountControlError, ProjfsEnableOutcome};

#[cfg(target_os = "linux")]
use steam_depot_mount::{Mount, MountConfig, MountError};
#[cfg(target_os = "macos")]
use steam_depot_mount::{NfsMount, NfsMountConfig};
#[cfg(target_os = "windows")]
use steam_depot_mount::{ProjFsMount, ProjFsMountConfig, ProjFsMountError};

/// Same stack the HTTP routes use, so what the mount pulls from the CDN is
/// counted and marked present like any other read. `TrackedChunkStore` sits
/// below the disk cache, so cache hits stay invisible.
pub(super) type ChunkStoreC = FsCacheStore<TrackedChunkStore<CdnChunkStore<SteamClient>>>;

#[cfg(target_os = "linux")]
pub(super) type Backend = Mount<ChunkStoreC>;
#[cfg(target_os = "macos")]
pub(super) type Backend = NfsMount<ChunkStoreC>;
#[cfg(target_os = "windows")]
pub(super) type Backend = ProjFsMount<ChunkStoreC>;

#[cfg(target_os = "linux")]
pub(super) async fn start_backend(
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

#[cfg(target_os = "linux")]
pub(super) async fn unmount_backend(mount: Backend) -> Result<(), MountControlError> {
    mount.unmount().map_err(MountControlError::Unmount)
}

/// Ensure `path` exists as an empty directory ready to be FUSE-mounted.
///
/// Crash-recovery: if a previous FUSE mount died, the kernel still has an
/// entry but `read_dir` returns ENOTCONN ("Transport endpoint is not
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
/// success, `None` if the path is a stale FUSE mount that needs recovery,
/// `Err` for any other I/O error.
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

/// Run `Mount::start` and flatten its single-variant error wrapper down to
/// `io::Error` so callers don't see "FUSE error: FUSE error: …".
#[cfg(target_os = "linux")]
fn start_fuse(
    mountpoint: &std::path::Path,
    rt: Handle,
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

/// The NFS back end creates the mountpoint itself and needs no runtime
/// handle — it is async all the way down.
#[cfg(target_os = "macos")]
pub(super) async fn start_backend(
    mountpoint: &std::path::Path,
    _rt: Handle,
) -> Result<Backend, MountControlError> {
    NfsMount::start(NfsMountConfig::new(mountpoint.to_path_buf()))
        .await
        .map_err(|e| MountControlError::Start(std::io::Error::other(e.to_string())))
}

#[cfg(target_os = "macos")]
pub(super) async fn unmount_backend(mount: Backend) -> Result<(), MountControlError> {
    mount
        .unmount()
        .await
        .map_err(|e| MountControlError::Unmount(std::io::Error::other(e.to_string())))
}

/// ProjFS virtualizes in-process; `start` returns synchronously and needs
/// the runtime handle to drive async CDN work from its callback threads.
#[cfg(target_os = "windows")]
pub(super) async fn start_backend(
    mountpoint: &std::path::Path,
    rt: Handle,
) -> Result<Backend, MountControlError> {
    prepare_mountpoint(mountpoint).map_err(MountControlError::PrepareMountpoint)?;
    match ProjFsMount::start(ProjFsMountConfig::new(mountpoint.to_path_buf()), rt) {
        Ok(m) => Ok(m),
        // The projfs crate reports a disabled feature as `Unsupported`.
        Err(ProjFsMountError::Start(io)) if io.kind() == std::io::ErrorKind::Unsupported => {
            Err(MountControlError::ProjFsNotEnabled)
        }
        Err(ProjFsMountError::Start(io)) => Err(MountControlError::Start(io)),
    }
}

#[cfg(target_os = "windows")]
pub(super) async fn unmount_backend(mount: Backend) -> Result<(), MountControlError> {
    // Stops virtualization; the root and its placeholders stay on disk and
    // are reused on the next start.
    mount.unmount();
    Ok(())
}

/// Ensure the virtualization root exists. ProjFS reuses one root across
/// start/stop cycles, so we keep any existing placeholders — they're a
/// valid cache of immutable depot content. (Deleting placeholders orphaned
/// by a previous run fails with ERROR_VIRTUALIZATION_TEMPORARILY_UNAVAILABLE,
/// os error 369.)
#[cfg(target_os = "windows")]
fn prepare_mountpoint(path: &std::path::Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(path)
}

/// Prompt for elevation (UAC) and enable ProjFS. Blocking — call it off the
/// async runtime. Only reachable on Windows, the only platform that
/// produces [`MountControlError::ProjFsNotEnabled`].
#[cfg(target_os = "windows")]
pub fn prompt_enable_projfs() -> Result<ProjfsEnableOutcome, MountControlError> {
    use steam_depot_mount::EnableOutcome;
    match steam_depot_mount::enable_feature_elevated() {
        Ok(EnableOutcome::Enabled) => Ok(ProjfsEnableOutcome::Enabled),
        Ok(EnableOutcome::EnabledRestartNeeded) => Ok(ProjfsEnableOutcome::RestartNeeded),
        Ok(EnableOutcome::Cancelled) => Ok(ProjfsEnableOutcome::Cancelled),
        Err(e) => Err(MountControlError::EnableProjFs(e)),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn prompt_enable_projfs() -> Result<ProjfsEnableOutcome, MountControlError> {
    unreachable!("ProjFsNotEnabled is only produced on Windows")
}
