// TODO(ai-review): review for style and correctness
//! Manifest→chunk graph backing the store-management routes. Captures only
//! membership; chunk *presence* is read live from
//! [`StoreIndex`](super::store_index::StoreIndex) at query time.

use std::collections::{HashMap, HashSet};

use steam_depot_vfs::{ChunkHash, DepotStore};

use crate::steam::{AppId, DepotId, ManifestId};

pub type ManifestKey = (AppId, DepotId, ManifestId);

pub struct ManifestNode {
    pub app_id: AppId,
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub creation_time: u32,
    /// Deduplicated; Steam can list the same chunk across several files.
    pub chunks: Vec<ChunkHash>,
}

pub struct StoreModel {
    pub manifests: Vec<ManifestNode>,
    /// Number of distinct manifests referencing each chunk.
    pub chunk_refs: HashMap<ChunkHash, u32>,
    /// CDN wire size of each chunk. Approximates the on-disk frame size
    /// within a few percent; used for full-download projections. Exact
    /// disk usage is read from the files themselves.
    pub chunk_size: HashMap<ChunkHash, u64>,
}

impl StoreModel {
    pub fn build(store: &DepotStore) -> Result<Self, std::io::Error> {
        let mut manifests = Vec::new();
        let mut chunk_refs: HashMap<ChunkHash, u32> = HashMap::new();
        let mut chunk_size: HashMap<ChunkHash, u64> = HashMap::new();

        for (app_id, depot_id, gid) in store.list_manifests()? {
            let m = match store.load_cached_manifest(app_id, depot_id, gid) {
                Ok(Some(m)) => m,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(app_id, depot_id, gid, %err, "failed to load cached manifest for store model");
                    continue;
                }
            };

            let mut seen = HashSet::new();
            let mut chunks = Vec::new();
            for file in &m.files {
                for chunk in file.chunks() {
                    if seen.insert(chunk.sha) {
                        chunks.push(chunk.sha);
                        chunk_size
                            .entry(chunk.sha)
                            .or_insert_with(|| u64::from(chunk.size_compressed));
                    }
                }
            }
            for sha in &chunks {
                *chunk_refs.entry(*sha).or_insert(0) += 1;
            }
            manifests.push(ManifestNode {
                app_id: AppId(app_id),
                depot_id: DepotId(depot_id),
                manifest_id: ManifestId(gid),
                creation_time: m.creation_time,
                chunks,
            });
        }

        Ok(Self {
            manifests,
            chunk_refs,
            chunk_size,
        })
    }

    /// Present chunks whose every referencing manifest lies within
    /// `delete_set`. Exact byte accounting is left to the caller, who can
    /// stat the files — `chunk_size` only approximates them.
    pub fn freed_by(
        &self,
        delete_set: &HashSet<ManifestKey>,
        is_present: impl Fn(&ChunkHash) -> bool,
    ) -> Vec<ChunkHash> {
        let mut refs_in_set: HashMap<ChunkHash, u32> = HashMap::new();
        for node in &self.manifests {
            if delete_set.contains(&(node.app_id, node.depot_id, node.manifest_id)) {
                for sha in &node.chunks {
                    *refs_in_set.entry(*sha).or_insert(0) += 1;
                }
            }
        }

        refs_in_set
            .into_iter()
            .filter(|(sha, in_set)| {
                let total = self.chunk_refs.get(sha).copied().unwrap_or(0);
                *in_set == total && is_present(sha)
            })
            .map(|(sha, _)| sha)
            .collect()
    }

    /// Chunks referenced by the manifests NOT in `excluded`. The store
    /// routes' unreferenced sweep runs it with the `delete_metadata` set,
    /// so one prune pass also removes those manifests' chunks.
    pub fn referenced_chunks_excluding(
        &self,
        excluded: &HashSet<ManifestKey>,
    ) -> HashSet<ChunkHash> {
        let mut refs = HashSet::new();
        for node in &self.manifests {
            if excluded.contains(&(node.app_id, node.depot_id, node.manifest_id)) {
                continue;
            }
            refs.extend(&node.chunks);
        }
        refs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(n: u8) -> ChunkHash {
        ChunkHash([n; 20])
    }

    /// Manifests 10 and 20 share chunk 2; 1 and 3 are exclusive.
    fn model() -> StoreModel {
        let node = |m: u64, chunks: Vec<ChunkHash>| ManifestNode {
            app_id: AppId(1),
            depot_id: DepotId(2),
            manifest_id: ManifestId(m),
            creation_time: 0,
            chunks,
        };
        StoreModel {
            manifests: vec![
                node(10, vec![sha(1), sha(2)]),
                node(20, vec![sha(2), sha(3)]),
            ],
            chunk_refs: [(sha(1), 1), (sha(2), 2), (sha(3), 1)]
                .into_iter()
                .collect(),
            chunk_size: [(sha(1), 100), (sha(2), 100), (sha(3), 100)]
                .into_iter()
                .collect(),
        }
    }

    fn key(m: u64) -> ManifestKey {
        (AppId(1), DepotId(2), ManifestId(m))
    }

    fn freed_set(
        model: &StoreModel,
        keys: &[ManifestKey],
        present: impl Fn(&ChunkHash) -> bool,
    ) -> HashSet<ChunkHash> {
        model
            .freed_by(&keys.iter().copied().collect(), present)
            .into_iter()
            .collect()
    }

    #[test]
    fn freed_is_non_additive_for_shared_chunks() {
        let model = model();
        let present = |_: &ChunkHash| true;

        let a = freed_set(&model, &[key(10)], present);
        let b = freed_set(&model, &[key(20)], present);
        let both = freed_set(&model, &[key(10), key(20)], present);

        // Order comes out of a HashMap; compare as sets.
        assert_eq!(a, [sha(1)].into_iter().collect());
        assert_eq!(b, [sha(3)].into_iter().collect());
        assert_eq!(both, [sha(1), sha(2), sha(3)].into_iter().collect());
    }

    #[test]
    fn absent_chunks_are_not_freed() {
        let model = model();
        // Only sha(1) on disk; sha(2)/sha(3) missing.
        let present = |s: &ChunkHash| *s == sha(1);
        let both = freed_set(&model, &[key(10), key(20)], present);
        assert_eq!(both, [sha(1)].into_iter().collect());
    }

    #[test]
    fn referenced_chunks_excluding_drops_only_the_excluded_manifests() {
        let model = model();

        let none: HashSet<ManifestKey> = HashSet::new();
        assert_eq!(
            model.referenced_chunks_excluding(&none),
            [sha(1), sha(2), sha(3)].into_iter().collect()
        );
        // sha(1) is only referenced by manifest 10.
        assert_eq!(
            model.referenced_chunks_excluding(&[key(10)].into_iter().collect()),
            [sha(2), sha(3)].into_iter().collect()
        );
        // sha(2) survives excluding one of its two referrers.
        assert_eq!(
            model.referenced_chunks_excluding(&[key(10), key(20)].into_iter().collect()),
            HashSet::new()
        );
    }
}
