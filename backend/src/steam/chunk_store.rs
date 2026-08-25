// TODO(ai-review): review for style and correctness
//! [`ChunkStore`] layer that routes every cache-miss fetch through the
//! central [`ChunkService`] (single-flight dedup, download accounting).
//!
//! Placed between [`CdnChunkStore`](steam_depot_vfs::chunk_store::CdnChunkStore)
//! and [`FsCacheStore`](steam_depot_vfs::chunk_store::FsCacheStore) so
//! disk-cache hits stay invisible.

use std::sync::Arc;

use steam_depot_vfs::ChunkHash;
use steam_depot_vfs::chunk_store::ChunkStore;

use crate::state::downloads::ChunkService;

pub struct TrackedChunkStore<Inner: ChunkStore> {
    inner: Inner,
    service: Arc<ChunkService>,
}

impl<Inner: ChunkStore> TrackedChunkStore<Inner> {
    pub fn new(inner: Inner, service: Arc<ChunkService>) -> Self {
        Self { inner, service }
    }
}

impl<Inner: ChunkStore> ChunkStore for TrackedChunkStore<Inner> {
    async fn get(
        &self,
        sha: ChunkHash,
    ) -> std::result::Result<bytes::Bytes, steam_depot_vfs::VfsError> {
        self.service.fetch_via(sha, self.inner.get(sha)).await
    }

    async fn ensure(&self, sha: ChunkHash) -> std::result::Result<(), steam_depot_vfs::VfsError> {
        // Route through get() so concurrent get/ensure of the same SHA
        // share one CDN request. In practice FsCacheStore only calls get().
        self.service.fetch_via(sha, self.inner.get(sha)).await?;
        Ok(())
    }
}
