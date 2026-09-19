// TODO(ai-review): review for style and correctness
//! Store-management: what's on disk per app/depot/manifest, and pruning it.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::json;
use steam_depot_vfs::ChunkHash;
use utoipa::ToSchema;

use crate::http::ApiError;
use crate::state::AppState;
use crate::state::store_model::ManifestKey;
use crate::steam::{AppId, DepotId, ManifestId};

use super::Result;

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "total_bytes_on_disk": 21474836480u64,
    "total_chunks_on_disk": 24000,
    "unreferenced": {"chunks": 12, "bytes": 8388608},
    "apps": []
}))]
pub struct StoreOverview {
    /// Distinct on-disk bytes across the whole store (shared chunks counted once).
    pub total_bytes_on_disk: u64,
    pub total_chunks_on_disk: u64,
    pub unreferenced: UnreferencedBucket,
    pub apps: Vec<StoreApp>,
}

/// Chunks on disk referenced by no cached manifest.
#[derive(Serialize, ToSchema)]
pub struct UnreferencedBucket {
    pub chunks: u64,
    pub bytes: u64,
}

#[derive(Serialize, ToSchema)]
pub struct StoreApp {
    pub app_id: AppId,
    /// Distinct on-disk bytes for this app (shared chunks counted once).
    pub bytes_on_disk: u64,
    pub depots: Vec<StoreDepot>,
}

#[derive(Serialize, ToSchema)]
pub struct StoreDepot {
    pub depot_id: DepotId,
    pub manifests: Vec<StoreManifest>,
}

#[derive(Serialize, ToSchema)]
pub struct StoreManifest {
    pub manifest_id: ManifestId,
    pub creation_time: u32,
    pub chunks_total: u32,
    pub chunks_present: u32,
    /// Compressed footprint if every chunk this manifest references were
    /// downloaded (wire size; the frames on disk track it within a few
    /// percent).
    pub bytes_total: u64,
    /// Bytes currently on disk for this manifest's chunks (actual file
    /// sizes).
    pub bytes_on_disk: u64,
    /// Bytes reclaimed by deleting only this manifest (its exclusive
    /// chunks, actual file sizes).
    pub bytes_unique: u64,
}

/// Store overview
#[utoipa::path(get, path = "/api/store", tag = "store", responses((status = 200, body = StoreOverview)))]
#[tracing::instrument(skip_all)]
pub async fn store_overview(State(state): State<AppState>) -> Result<Json<StoreOverview>> {
    let model = state.store_model()?;

    // Cloned out of the index so the read lock is free before the stat
    // calls: the same process serves the FUSE mount on this runtime.
    let present: HashSet<ChunkHash> = {
        let index = state.store_index.read().expect("store_index poisoned");
        index.present_chunks().clone()
    };

    // Actual file sizes: `chunk_size` (wire size) only approximates the
    // frames, and this route exists to state disk usage.
    let chunks_root = state.store.chunks_root();
    let file_size: HashMap<ChunkHash, u64> = present
        .iter()
        .map(|sha| {
            let len = std::fs::metadata(chunks_root.join(sha.to_string()))
                .map(|m| m.len())
                .unwrap_or(0);
            (*sha, len)
        })
        .collect();

    let mut apps: BTreeMap<u32, BTreeMap<u32, Vec<StoreManifest>>> = BTreeMap::new();
    // Union per app, so a shared chunk counts once toward the app total.
    let mut app_present: HashMap<u32, HashSet<ChunkHash>> = HashMap::new();

    for node in &model.manifests {
        let mut chunks_present = 0u32;
        let mut bytes_total = 0u64;
        let mut bytes_on_disk = 0u64;
        let mut bytes_unique = 0u64;
        let app_set = app_present.entry(node.app_id.0).or_default();
        for sha in &node.chunks {
            bytes_total += model.chunk_size.get(sha).copied().unwrap_or(0);
            if let Some(size) = file_size.get(sha) {
                chunks_present += 1;
                bytes_on_disk += size;
                app_set.insert(*sha);
                if model.chunk_refs.get(sha).copied() == Some(1) {
                    bytes_unique += size;
                }
            }
        }
        apps.entry(node.app_id.0)
            .or_default()
            .entry(node.depot_id.0)
            .or_default()
            .push(StoreManifest {
                manifest_id: node.manifest_id,
                creation_time: node.creation_time,
                chunks_total: node.chunks.len() as u32,
                chunks_present,
                bytes_total,
                bytes_on_disk,
                bytes_unique,
            });
    }

    let mut total_ref_bytes = 0u64;
    let mut unref = UnreferencedBucket {
        chunks: 0,
        bytes: 0,
    };
    for (sha, size) in &file_size {
        if model.chunk_refs.contains_key(sha) {
            total_ref_bytes += size;
        } else {
            unref.chunks += 1;
            unref.bytes += size;
        }
    }

    let total_chunks_on_disk = present.len() as u64;

    let mut apps: Vec<StoreApp> = apps
        .into_iter()
        .map(|(app_id, depots)| {
            let bytes_on_disk = app_present
                .get(&app_id)
                .map(|set| {
                    set.iter()
                        .map(|s| file_size.get(s).copied().unwrap_or(0))
                        .sum()
                })
                .unwrap_or(0);
            let mut depots: Vec<StoreDepot> = depots
                .into_iter()
                .map(|(depot_id, mut manifests)| {
                    manifests.sort_by_key(|manifest| Reverse(manifest.creation_time));
                    StoreDepot {
                        depot_id: DepotId(depot_id),
                        manifests,
                    }
                })
                .collect();
            depots.sort_by_key(|d| d.depot_id.0);
            StoreApp {
                app_id: AppId(app_id),
                bytes_on_disk,
                depots,
            }
        })
        .collect();
    apps.sort_by_key(|app| Reverse(app.bytes_on_disk));

    Ok(Json(StoreOverview {
        total_bytes_on_disk: total_ref_bytes + unref.bytes,
        total_chunks_on_disk,
        unreferenced: unref,
        apps,
    }))
}

#[derive(Deserialize, ToSchema)]
pub struct StoreManifestRef {
    pub app_id: AppId,
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
}

#[derive(Deserialize, ToSchema)]
pub struct PruneRequest {
    /// Free the exclusive chunks of these manifests (postcards kept). A chunk
    /// is freed only when every manifest referencing it is in this set.
    #[serde(default)]
    pub free_chunks: Vec<StoreManifestRef>,
    /// Delete these manifests' metadata (postcard). Their chunks stay on
    /// disk, and the `include_unreferenced` sweep treats them as already
    /// gone, so one pass removes metadata and chunks together.
    #[serde(default)]
    pub delete_metadata: Vec<StoreManifestRef>,
    /// Also delete chunks referenced by no cached manifest
    /// (`delete_metadata` manifests count as already gone).
    #[serde(default)]
    pub include_unreferenced: bool,
}

#[derive(Serialize, ToSchema)]
pub struct PruneResult {
    pub freed_bytes: u64,
    pub freed_chunks: u64,
}

fn delete_set(refs: &[StoreManifestRef]) -> HashSet<ManifestKey> {
    refs.iter()
        .map(|r| (r.app_id, r.depot_id, r.manifest_id))
        .collect()
}

/// Prune preview
#[utoipa::path(post, path = "/api/store/prune/preview", tag = "store", request_body = PruneRequest, responses((status = 200, body = PruneResult)))]
#[tracing::instrument(skip_all, fields(count = body.free_chunks.len()))]
pub async fn prune_preview(
    State(state): State<AppState>,
    Json(body): Json<PruneRequest>,
) -> Result<Json<PruneResult>> {
    let model = state.store_model()?;
    // The same chunk sets the prune will delete (freed and orphan sets are
    // disjoint: freed chunks are excluded from the sweep) so preview and
    // result agree even where the frames deviate from the wire size.
    let (freed, orphans): (Vec<ChunkHash>, Vec<ChunkHash>) = {
        let index = state.store_index.read().expect("store_index poisoned");
        let freed = model.freed_by(&delete_set(&body.free_chunks), |s| index.has_chunk(s));
        let orphans = if body.include_unreferenced {
            let kept = model.referenced_chunks_excluding(&delete_set(&body.delete_metadata));
            index
                .present_chunks()
                .iter()
                .filter(|s| !kept.contains(*s) && !freed.contains(s))
                .copied()
                .collect()
        } else {
            Vec::new()
        };
        (freed, orphans)
    };
    let chunks_root = state.store.chunks_root();
    let mut freed_bytes = 0u64;
    for sha in freed.iter().chain(&orphans) {
        freed_bytes += std::fs::metadata(chunks_root.join(sha.to_string()))
            .map(|m| m.len())
            .unwrap_or(0);
    }
    Ok(Json(PruneResult {
        freed_bytes,
        freed_chunks: (freed.len() + orphans.len()) as u64,
    }))
}

/// Prune
#[utoipa::path(post, path = "/api/store/prune", tag = "store", request_body = PruneRequest, responses((status = 200, body = PruneResult)))]
#[tracing::instrument(skip_all, fields(free = body.free_chunks.len(), metadata = body.delete_metadata.len(), unreferenced = body.include_unreferenced))]
pub async fn prune(
    State(state): State<AppState>,
    Json(body): Json<PruneRequest>,
) -> Result<Json<PruneResult>> {
    let model = state.store_model()?;
    let chunks_root = state.store.chunks_root();

    let freed = {
        let index = state.store_index.read().expect("store_index poisoned");
        model.freed_by(&delete_set(&body.free_chunks), |s| index.has_chunk(s))
    };

    let mut freed_bytes = 0u64;
    let mut freed_chunks = 0u64;
    for sha in &freed {
        freed_bytes += std::fs::metadata(chunks_root.join(sha.to_string()))
            .map(|m| m.len())
            .unwrap_or(0);
        remove_chunk(&chunks_root, sha)?;
        freed_chunks += 1;
    }

    let orphans: Vec<ChunkHash> = if body.include_unreferenced {
        let index = state.store_index.read().expect("store_index poisoned");
        let kept = model.referenced_chunks_excluding(&delete_set(&body.delete_metadata));
        index
            .present_chunks()
            .iter()
            .filter(|s| !kept.contains(*s) && !freed.contains(s))
            .copied()
            .collect()
    } else {
        Vec::new()
    };
    for sha in &orphans {
        let size = std::fs::metadata(chunks_root.join(sha.to_string()))
            .map(|m| m.len())
            .unwrap_or(0);
        remove_chunk(&chunks_root, sha)?;
        freed_bytes += size;
        freed_chunks += 1;
    }

    {
        let mut index = state.store_index.write().expect("store_index poisoned");
        for sha in freed.iter().chain(&orphans) {
            index.mark_chunk_absent(sha);
        }
    }

    if !body.delete_metadata.is_empty() {
        let manifests_root = state.store.manifests_root();
        for (app_id, depot_id, manifest_id) in delete_set(&body.delete_metadata) {
            let path = manifests_root
                .join(app_id.to_string())
                .join(depot_id.to_string())
                .join(format!("{manifest_id}.postcard"));
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(ApiError::internal(format!("removing {path:?}: {e}"))),
            }
        }
        state.invalidate_store_model();
    }

    tracing::info!(freed_bytes, freed_chunks, "store pruned");
    Ok(Json(PruneResult {
        freed_bytes,
        freed_chunks,
    }))
}

fn remove_chunk(chunks_root: &std::path::Path, sha: &ChunkHash) -> Result<()> {
    match std::fs::remove_file(chunks_root.join(sha.to_string())) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ApiError::internal(format!("removing chunk {sha}: {e}"))),
    }
}
