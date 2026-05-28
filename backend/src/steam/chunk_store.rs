// TODO(ai-review): review for style and correctness
//! Instrumented [`ChunkStore`] wrapper that reports every fetch into
//! the [`DownloadManager`] so the UI's downloads drawer covers out-of-
//! band reads too (rabex-env's implicit bundle/typetree loads, etc.).

use std::sync::Arc;

use steam_depot_vfs::ChunkHash;
use steam_depot_vfs::chunk_store::ChunkStore;

use crate::state::downloads::DownloadManager;

/// `ChunkStore` wrapper that pipes every fetch through the
/// [`DownloadManager`] so out-of-band reads — anything that doesn't
/// go through [`DownloadManager::enqueue`], typically rabex-env's
/// implicit bundle/typetree loads — still surface in the downloads
/// drawer with live byte counters and ETA.
///
/// Placed between [`CdnChunkStore`](steam_depot_vfs::chunk_store::CdnChunkStore)
/// and [`FsCacheStore`](steam_depot_vfs::chunk_store::FsCacheStore) so
/// that disk-cache hits stay invisible (and don't pollute the speed
/// average with zero-duration "fetches"). Construction is done in
/// [`crate::state::AppState::open_manifest`] via
/// `DepotStore::open_depot_manifest_with_chunks`.
///
/// Errors and the returned bytes propagate untouched — this is purely
/// observational.
pub struct TrackedChunkStore<Inner: ChunkStore> {
    inner: Inner,
    downloads: Arc<DownloadManager>,
}

impl<Inner: ChunkStore> TrackedChunkStore<Inner> {
    pub fn new(inner: Inner, downloads: Arc<DownloadManager>) -> Self {
        Self { inner, downloads }
    }
}

impl<Inner: ChunkStore> ChunkStore for TrackedChunkStore<Inner> {
    async fn get(
        &self,
        sha: ChunkHash,
    ) -> std::result::Result<bytes::Bytes, steam_depot_vfs::VfsError> {
        self.downloads.track_fetch_started(sha);
        let res = self.inner.get(sha).await;
        let (report, size) = match &res {
            Ok(b) => (Ok(()), b.len() as u64),
            Err(e) => (Err(clone_vfs_error(e)), 0),
        };
        self.downloads.track_fetch_completed(sha, size, report);
        res
    }

    async fn ensure(&self, sha: ChunkHash) -> std::result::Result<(), steam_depot_vfs::VfsError> {
        self.downloads.track_fetch_started(sha);
        let res = self.inner.ensure(sha).await;
        // ensure() doesn't return the bytes; we don't know the size.
        // Report 0 so chunks_completed still ticks but bytes_completed
        // is honest about not having a number.
        let report = match &res {
            Ok(()) => Ok(()),
            Err(e) => Err(clone_vfs_error(e)),
        };
        self.downloads.track_fetch_completed(sha, 0, report);
        res
    }
}

/// `VfsError` doesn't implement `Clone`, but the `track_fetch_completed`
/// signature wants an owned value alongside the original being
/// propagated. Stringifying preserves the message which is all the
/// stats path uses (`s.last_error = Some(err.to_string())`).
fn clone_vfs_error(err: &steam_depot_vfs::VfsError) -> steam_depot_vfs::VfsError {
    steam_depot_vfs::VfsError::Other(err.to_string().into())
}
