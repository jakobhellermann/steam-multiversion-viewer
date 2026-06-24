//! Materializes a manifest into a real directory tree on disk.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use camino::{Utf8Path, Utf8PathBuf};
use futures_util::FutureExt as _;
use serde::Serialize;
use steam_vent_depot::{FileKind, Manifest};
use tokio::io::AsyncWriteExt;
use utoipa::ToSchema;

use super::Snapshot;
use super::downloads::DownloadManager;
use crate::steam::{AppId, DepotId, ManifestId};

/// Window size for the read-then-write loop, so a multi-GiB file never lands in memory whole.
const WRITE_WINDOW: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportState {
    #[default]
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct ExportTarget {
    pub app_id: AppId,
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
}

#[derive(Clone, Debug, Default, Serialize, ToSchema)]
pub struct ExportStatus {
    pub state: ExportState,
    pub target: Option<ExportTarget>,
    #[schema(value_type = Option<String>)]
    pub target_dir: Option<Utf8PathBuf>,
    pub files_total: u64,
    pub files_done: u64,
    pub bytes_total: u64,
    pub bytes_written: u64,
    pub current_path: Option<String>,
    pub error: Option<String>,
}

pub struct ExportManager {
    status: Mutex<ExportStatus>,
    downloads: Arc<DownloadManager>,
    cancelled: AtomicBool,
}

#[derive(Debug)]
pub enum ExportError {
    AlreadyRunning,
    UnsafePath(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("an export is already running"),
            Self::UnsafePath(p) => write!(f, "manifest contains an unsafe path: {p}"),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl ExportManager {
    pub fn new(downloads: Arc<DownloadManager>) -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(ExportStatus::default()),
            downloads,
            cancelled: AtomicBool::new(false),
        })
    }

    pub fn current(&self) -> ExportStatus {
        self.status.lock().expect("export status poisoned").clone()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Claim the single export slot and drive the export in the background.
    pub fn start(
        self: &Arc<Self>,
        snapshot: Arc<Snapshot>,
        target: ExportTarget,
        target_dir: Utf8PathBuf,
        launch_targets: Vec<String>,
    ) -> Result<ExportStatus, ExportError> {
        let plan = Plan::build(snapshot.manifest(), &launch_targets)?;

        let initial = {
            let mut status = self.status.lock().expect("export status poisoned");
            if status.state == ExportState::Running {
                return Err(ExportError::AlreadyRunning);
            }
            *status = ExportStatus {
                state: ExportState::Running,
                target: Some(target),
                target_dir: Some(target_dir.clone()),
                files_total: plan.files.len() as u64,
                bytes_total: plan.bytes_total,
                ..Default::default()
            };
            status.clone()
        };
        self.cancelled.store(false, Ordering::Release);

        let this = Arc::clone(self);
        tokio::spawn(async move {
            // Without this a panic would skip the bookkeeping below and leave
            // the export stuck on `Running`, blocking every later one.
            let run = AssertUnwindSafe(this.run(&snapshot, &target_dir, plan));
            let result = run.catch_unwind().await.unwrap_or_else(|_| {
                Err(ExportError::Io(std::io::Error::other(
                    "export panicked; see the log for the backtrace",
                )))
            });
            let final_status = {
                let mut status = this.status.lock().expect("export status poisoned");
                status.current_path = None;
                status.state = match &result {
                    Ok(()) if this.cancelled.load(Ordering::Acquire) => ExportState::Cancelled,
                    Ok(()) => ExportState::Done,
                    Err(err) => {
                        status.error = Some(err.to_string());
                        ExportState::Failed
                    }
                };
                status.clone()
            };
            match &result {
                Ok(()) => tracing::info!(
                    target_dir = %target_dir,
                    files = final_status.files_done,
                    bytes = final_status.bytes_written,
                    state = ?final_status.state,
                    "export finished"
                ),
                Err(err) => tracing::error!(target_dir = %target_dir, %err, "export failed"),
            }
        });

        Ok(initial)
    }

    async fn run(
        &self,
        snapshot: &Arc<Snapshot>,
        target_dir: &Utf8Path,
        plan: Plan,
    ) -> Result<(), ExportError> {
        tokio::fs::create_dir_all(target_dir).await?;
        for dir in &plan.dirs {
            tokio::fs::create_dir_all(target_dir.join(dir)).await?;
        }

        // Queue every chunk up front so the download worker's parallelism is
        // saturated while we write files out one by one.
        self.downloads
            .enqueue(Arc::clone(snapshot), plan.all_chunks())
            .await;

        let mut buf = Vec::with_capacity(WRITE_WINDOW as usize);
        for file in &plan.files {
            if self.cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            self.update(|s| s.current_path = Some(file.path.clone()));

            let dest = target_dir.join(&file.path);
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            self.downloads
                .enqueue_and_wait(Arc::clone(snapshot), file.chunks.iter().copied())
                .await;

            let mut out = tokio::fs::File::create(&dest).await?;
            let mut offset = 0;
            while offset < file.size {
                buf.clear();
                snapshot
                    .read_into(&file.path, offset, WRITE_WINDOW, &mut buf)
                    .await?;
                if buf.is_empty() {
                    return Err(ExportError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        format!("{} ended at {offset} of {} bytes", file.path, file.size),
                    )));
                }
                out.write_all(&buf).await?;
                offset += buf.len() as u64;
                let written = buf.len() as u64;
                self.update(|s| s.bytes_written += written);
            }
            out.flush().await?;
            set_executable(&out, file.executable).await?;
            self.update(|s| s.files_done += 1);
        }

        for link in &plan.symlinks {
            create_symlink(&link.target, &target_dir.join(&link.path))?;
        }
        Ok(())
    }

    fn update(&self, f: impl FnOnce(&mut ExportStatus)) {
        f(&mut self.status.lock().expect("export status poisoned"));
    }
}

struct Plan {
    dirs: Vec<String>,
    files: Vec<PlanFile>,
    symlinks: Vec<PlanSymlink>,
    bytes_total: u64,
}

struct PlanFile {
    path: String,
    size: u64,
    executable: bool,
    chunks: Vec<(steam_depot_vfs::ChunkHash, u64)>,
}

struct PlanSymlink {
    path: String,
    target: String,
}

impl Plan {
    fn build(manifest: &Manifest, launch_targets: &[String]) -> Result<Self, ExportError> {
        let runnable = RunnablePaths::new(manifest, launch_targets);
        let mut plan = Self {
            dirs: Vec::new(),
            files: Vec::new(),
            symlinks: Vec::new(),
            bytes_total: 0,
        };
        for f in &manifest.files {
            let path = check_relative_path(&f.path)?;
            match f.kind {
                FileKind::Directory => plan.dirs.push(path.to_owned()),
                FileKind::File => {
                    plan.bytes_total += f.size;
                    plan.files.push(PlanFile {
                        path: path.to_owned(),
                        size: f.size,
                        executable: f.executable || runnable.contains(path),
                        chunks: f
                            .chunks
                            .iter()
                            .map(|c| (c.sha, u64::from(c.size_compressed)))
                            .collect(),
                    });
                }
                FileKind::Symlink => {
                    let target = f
                        .linktarget
                        .clone()
                        .ok_or_else(|| ExportError::UnsafePath(f.path.clone()))?;
                    plan.symlinks.push(PlanSymlink {
                        path: path.to_owned(),
                        target,
                    });
                }
            }
        }
        Ok(plan)
    }

    fn all_chunks(&self) -> Vec<(steam_depot_vfs::ChunkHash, u64)> {
        self.files
            .iter()
            .flat_map(|f| f.chunks.iter().copied())
            .collect()
    }
}

/// Paths the app's launch config names as runnable. A macOS launch target is
/// the `.app` bundle rather than a file, so everything directly inside its
/// `Contents/MacOS/` counts instead.
struct RunnablePaths(HashSet<String>);

impl RunnablePaths {
    fn new(manifest: &Manifest, launch_targets: &[String]) -> Self {
        let mut paths = HashSet::new();
        for target in launch_targets {
            let target = target.trim_end_matches('/').replace('\\', "/");
            let bundle_bin = format!("{target}/Contents/MacOS/");
            for f in &manifest.files {
                if !matches!(f.kind, FileKind::File) {
                    continue;
                }
                let in_bundle = f
                    .path
                    .strip_prefix(&bundle_bin)
                    .is_some_and(|rest| !rest.contains('/'));
                if f.path == target || in_bundle {
                    paths.insert(f.path.clone());
                }
            }
        }
        Self(paths)
    }

    fn contains(&self, path: &str) -> bool {
        self.0.contains(path)
    }
}

/// Rejects anything that isn't a single directory name.
pub fn check_single_component(name: &str) -> Result<&str, ExportError> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(name),
        _ => Err(ExportError::UnsafePath(name.to_owned())),
    }
}

/// Rejects anything that could escape the export root — only plain names, no `..` or roots.
fn check_relative_path(path: &str) -> Result<&str, ExportError> {
    let mut components = Path::new(path).components().peekable();
    let relative =
        components.peek().is_some() && components.all(|c| matches!(c, Component::Normal(_)));
    match relative {
        true => Ok(path),
        false => Err(ExportError::UnsafePath(path.to_owned())),
    }
}

#[cfg(unix)]
async fn set_executable(file: &tokio::fs::File, executable: bool) -> Result<(), ExportError> {
    use std::os::unix::fs::PermissionsExt;
    if !executable {
        return Ok(());
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o755))
        .await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_executable(_file: &tokio::fs::File, _executable: bool) -> Result<(), ExportError> {
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &str, at: &Utf8Path) -> Result<(), ExportError> {
    match std::os::unix::fs::symlink(target, at) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        res => Ok(res?),
    }
}

#[cfg(not(unix))]
fn create_symlink(_target: &str, at: &Utf8Path) -> Result<(), ExportError> {
    Err(ExportError::Io(std::io::Error::other(format!(
        "manifest contains a symlink ({at}), which this platform can't create"
    ))))
}

#[cfg(test)]
mod tests {
    use super::{RunnablePaths, check_relative_path, check_single_component};
    use steam_vent_depot::{DepotFile, FileKind, Manifest};

    fn manifest(paths: &[(&str, FileKind)]) -> Manifest {
        Manifest {
            depot_id: 1,
            manifest_id: 1,
            creation_time: 0,
            size_uncompressed: 0,
            size_compressed: 0,
            files: paths
                .iter()
                .map(|(path, kind)| DepotFile {
                    path: (*path).to_owned(),
                    size: 0,
                    kind: *kind,
                    executable: false,
                    sha: None,
                    linktarget: None,
                    chunks: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn macos_launch_target_resolves_to_the_bundle_binary() {
        let m = manifest(&[
            ("game.app", FileKind::Directory),
            ("game.app/Contents/MacOS/game", FileKind::File),
            ("game.app/Contents/MacOS/helper", FileKind::File),
            ("game.app/Contents/MacOS/nested/deep", FileKind::File),
            ("game.app/Contents/Info.plist", FileKind::File),
            ("game.app/Contents/Resources/data", FileKind::File),
        ]);
        let runnable = RunnablePaths::new(&m, &["game.app".into()]);

        assert!(runnable.contains("game.app/Contents/MacOS/game"));
        assert!(runnable.contains("game.app/Contents/MacOS/helper"));
        // Only direct children of MacOS/, and nothing outside it.
        assert!(!runnable.contains("game.app/Contents/MacOS/nested/deep"));
        assert!(!runnable.contains("game.app/Contents/Info.plist"));
        assert!(!runnable.contains("game.app/Contents/Resources/data"));
    }

    #[test]
    fn plain_launch_target_matches_the_file_itself() {
        let m = manifest(&[
            ("game.x86_64", FileKind::File),
            ("game.exe", FileKind::File),
            ("data/blob", FileKind::File),
        ]);
        // Targets for other platforms simply don't exist in this manifest.
        let runnable = RunnablePaths::new(&m, &["game.x86_64".into(), "other.exe".into()]);

        assert!(runnable.contains("game.x86_64"));
        assert!(!runnable.contains("game.exe"));
        assert!(!runnable.contains("data/blob"));
    }

    #[test]
    fn rejects_escaping_paths() {
        for path in ["", "/etc/passwd", "..", "a/../../b", "/"] {
            assert!(check_relative_path(path).is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn accepts_normal_paths() {
        for path in ["a", "a/b/c.txt", "Game_Data/level0"] {
            assert_eq!(check_relative_path(path).unwrap(), path);
        }
    }

    #[test]
    fn single_component_rejects_nesting_and_traversal() {
        for name in ["", ".", "..", "a/b", "/a", "../a"] {
            assert!(check_single_component(name).is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn single_component_accepts_plain_names() {
        for name in ["Hollow Knight-1.5.78.11833", "367520-367523-58295332"] {
            assert_eq!(check_single_component(name).unwrap(), name);
        }
    }
}
