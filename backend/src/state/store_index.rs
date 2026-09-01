// TODO(ai-review): review for style and correctness
//! In-memory index over the on-disk depot store: [`StoreIndex::chunks_present`]
//! tracks the set of chunk SHAs we actually have on disk. Populated at
//! startup, and updated as manifests are opened and chunks come and go.

use std::collections::HashSet;

use steam_depot_vfs::{ChunkHash, DepotStore};
use steam_vent_depot::Manifest;

use crate::steam::{AppId, DepotId, ManifestId};

pub struct StoreIndex {
    chunks_present: HashSet<ChunkHash>,
    indexed_manifests: HashSet<(AppId, DepotId, ManifestId)>,
}

impl StoreIndex {
    /// Build by enumerating chunks and manifests already on disk.
    pub fn scan(store: &DepotStore) -> Result<Self, std::io::Error> {
        let started = std::time::Instant::now();
        let mut idx = Self {
            chunks_present: HashSet::new(),
            indexed_manifests: HashSet::new(),
        };

        for hash in store.list_chunks()? {
            idx.chunks_present.insert(hash?);
        }

        for (app_id_raw, depot_id_raw, gid_raw) in store.list_manifests()? {
            match store.load_cached_manifest(app_id_raw, depot_id_raw, gid_raw) {
                Ok(Some(m)) => {
                    idx.add_manifest(AppId(app_id_raw), &m);
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(
                        app_id = app_id_raw,
                        depot_id = depot_id_raw,
                        gid = gid_raw,
                        %err,
                        "failed to load cached manifest while indexing"
                    );
                }
            }
        }

        tracing::info!(
            chunks_present = idx.chunks_present.len(),
            manifests = idx.indexed_manifests.len(),
            time = ?started.elapsed(),
            "store index built"
        );
        Ok(idx)
    }

    /// Record a manifest as indexed. Returns `true` if this was a new entry,
    /// `false` if we'd already indexed this `(app_id, depot_id, manifest_id)`
    /// triple.
    pub fn add_manifest(&mut self, app_id: AppId, m: &Manifest) -> bool {
        self.indexed_manifests
            .insert((app_id, DepotId(m.depot_id), ManifestId(m.manifest_id)))
    }

    /// Iterate every indexed `(app, depot, manifest)` triple.
    pub fn indexed(&self) -> impl Iterator<Item = (AppId, DepotId, ManifestId)> + '_ {
        self.indexed_manifests.iter().copied()
    }

    /// Record that a chunk now exists on disk.
    pub fn mark_chunk_present(&mut self, sha: ChunkHash) {
        self.chunks_present.insert(sha);
    }

    /// Record that a chunk was removed from disk.
    pub fn mark_chunk_absent(&mut self, sha: &ChunkHash) {
        self.chunks_present.remove(sha);
    }

    pub fn has_chunk(&self, sha: &ChunkHash) -> bool {
        self.chunks_present.contains(sha)
    }

    /// The set of chunks currently on disk.
    pub fn present_chunks(&self) -> &HashSet<ChunkHash> {
        &self.chunks_present
    }

    /// Aggregate stats for a single manifest against the current indexes.
    pub fn manifest_stats(&self, m: &Manifest) -> ManifestStats {
        // Deduplicate by chunk SHA within the manifest — Steam can list the
        // same chunk in multiple files (e.g. as a shared section), and we
        // don't want to count its bytes twice.
        let mut seen = HashSet::new();
        let mut s = ManifestStats::default();
        for file in &m.files {
            for chunk in file.chunks() {
                if !seen.insert(chunk.sha) {
                    continue;
                }
                let on_disk = u64::from(chunk.size_uncompressed);
                let wire = u64::from(chunk.size_compressed);
                s.chunks_total += 1;
                s.bytes_total += on_disk;
                s.bytes_total_compressed += wire;
                if !self.chunks_present.contains(&chunk.sha) {
                    s.chunks_missing += 1;
                    s.bytes_missing_compressed += wire;
                }
            }
        }
        s
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ManifestStats {
    pub chunks_total: u32,
    pub chunks_missing: u32,
    /// Uncompressed bytes — total disk footprint when fully downloaded.
    pub bytes_total: u64,
    /// Compressed bytes — total download size of the whole manifest.
    pub bytes_total_compressed: u64,
    /// Compressed bytes you'd pull over the wire from Steam's CDN to
    /// complete the download.
    pub bytes_missing_compressed: u64,
}
