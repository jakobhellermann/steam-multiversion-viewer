// TODO(ai-review): review for style and correctness
//! Manifest-level path diffs: which paths changed, were added, or exist only in the compare targets.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse as _, Response};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::json;
use steam_depot_vfs::VfsError;
use steam_vent_depot::{DepotFile, DepotFileKind, FileHash};
use tokio::sync::Semaphore;
use utoipa::ToSchema;

use crate::state::{AppState, Snapshot};
use crate::steam::{AppId, DepotId, ManifestId};

use crate::http::{CacheSeconds, ImmutableCache};
use crate::routes::Result;
use crate::routes::library::ManifestRef;

#[derive(Deserialize, ToSchema)]
pub struct ManifestDiffRequest {
    /// The manifest whose paths we report; a path counts if changed vs any `other` or absent from all of them.
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "entries": [
        {"path": "Data/Engine.dll",      "status": "changed"},
        {"path": "Data/NewModule.dll",   "status": "added"},
        {"path": "Data/OldModule.dll",   "status": "removed", "size": 456789}
    ]
}))]
pub struct ManifestDiffResponse {
    /// Every path that differs: `Changed`/`Added` for paths in `base`, `Removed` for paths in every `other` but in no `base`.
    pub entries: Vec<ManifestDiffEntry>,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestDiffEntry {
    pub path: String,
    pub status: ManifestDiffStatus,
    /// Set on `Removed` rows: the file's size in the first `other` carrying the path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ManifestDiffStatus {
    /// In `base`, in *no* `other`.
    Added,
    /// In `base` and differs from some `other`; wins over `Added` when both apply.
    Changed,
    /// In *every* `other`, in no `base` — the mirror of `Added`; carries the `other`-side size.
    Removed,
}

/// Manifest path-level diff
///
/// `Changed` (differs from some `other`), `Added` (absent from every `other`), or `Removed` (in every `other`, in no `base`); identity is the content sha, or the symlink target ([`content_id`]).
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/diff",
    tag = "diff",
    request_body = ManifestDiffRequest,
    responses((status = 200, body = ManifestDiffResponse))
)]
#[tracing::instrument(skip_all, fields(others = body.others.len()))]
pub async fn manifest_diff(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ManifestDiffRequest>,
) -> Result<Json<ManifestDiffResponse>> {
    state.steam()?; // 401 if not logged in
    let base_snap = state
        .open_manifest(
            appid,
            body.base.depot_id,
            body.base.manifest_id,
            &body.base.branch,
        )
        .await?;

    // Comparing a manifest against itself yields nothing.
    let mut others = open_manifests_concurrently(
        &state,
        appid,
        &body.others,
        Some((body.base.depot_id, body.base.manifest_id)),
    );

    let base = base_snap.manifest();
    let mut base_by_path: HashMap<&str, &DepotFile> = HashMap::with_capacity(base.files.len());
    for f in &base.files {
        if f.is_dir() {
            continue;
        }
        base_by_path.insert(f.path.as_str(), f);
    }

    // `Changed` wins over `Added`: added-vs-A-but-changed-vs-B is still demonstrably different somewhere.
    let mut absent_in_all: HashSet<&str> = base_by_path.keys().copied().collect();
    let mut changed: HashSet<String> = HashSet::new();
    // Paths seen in every `other` so far, with the size from the first one carrying them.
    let mut in_every_other: Option<HashMap<String, u64>> = None;
    while let Some((_, result)) = others.next().await {
        let snap = result?;
        let other = snap.manifest();
        let mut paths: HashMap<String, u64> = HashMap::with_capacity(other.files.len());
        for f in &other.files {
            if f.is_dir() {
                continue;
            }
            absent_in_all.remove(f.path.as_str());
            paths.insert(f.path.clone(), f.size);
            if let Some(base_f) = base_by_path.get(f.path.as_str())
                && content_id(base_f) != content_id(f)
            {
                changed.insert(f.path.clone());
            }
        }
        in_every_other = Some(match in_every_other {
            None => paths,
            Some(mut kept) => {
                kept.retain(|p, _| paths.contains_key(p));
                kept
            }
        });
    }

    let mut entries: Vec<ManifestDiffEntry> =
        Vec::with_capacity(changed.len() + absent_in_all.len());
    for path in &changed {
        entries.push(ManifestDiffEntry {
            path: path.clone(),
            status: ManifestDiffStatus::Changed,
            size: None,
        });
    }
    for path in &absent_in_all {
        // Changed wins: skip paths already reported as changed.
        if changed.contains(*path) {
            continue;
        }
        entries.push(ManifestDiffEntry {
            path: (*path).to_owned(),
            status: ManifestDiffStatus::Added,
            size: None,
        });
    }
    for (path, size) in in_every_other.into_iter().flatten() {
        if base_by_path.contains_key(path.as_str()) {
            continue;
        }
        entries.push(ManifestDiffEntry {
            path,
            status: ManifestDiffStatus::Removed,
            size: Some(size),
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(ManifestDiffResponse { entries }))
}

#[derive(Deserialize, ToSchema)]
pub struct FileDiffTargetsRequest {
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
    pub path: String,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "statuses": [
        {"depot_id": 1234567, "manifest_id": "9876543210987654321", "status": "different"},
        {"depot_id": 1234567, "manifest_id": "1111222233334444555",  "status": "same"},
        {"depot_id": 1234567, "manifest_id": "5555666677778888999",  "status": "missing"}
    ]
}))]
pub struct FileDiffTargetsResponse {
    /// Per `other`: `same`, `different`, or `missing`; identity is the content sha, or the symlink target.
    pub statuses: Vec<FileDiffTargetStatus>,
}

#[derive(Serialize, ToSchema)]
pub struct FileDiffTargetStatus {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub status: FileDiffStatus,
}

#[derive(Serialize, ToSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum FileDiffStatus {
    Same,
    Different,
    Missing,
}

/// Find manifests with changes
///
/// For one file (`path`) against N targets, report `same`/`different`/`missing` per target.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/file/diff-targets",
    tag = "diff",
    request_body = FileDiffTargetsRequest,
    responses((status = 200, body = FileDiffTargetsResponse))
)]
#[tracing::instrument(skip_all, fields(path = %body.path, others = body.others.len()))]
pub async fn file_diff_targets(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<FileDiffTargetsRequest>,
) -> Result<Json<FileDiffTargetsResponse>> {
    state.steam()?; // 401 if not logged in
    let base_snap = state
        .open_manifest(
            appid,
            body.base.depot_id,
            body.base.manifest_id,
            &body.base.branch,
        )
        .await?;
    let base_file = base_snap
        .manifest()
        .files
        .iter()
        .find(|f| f.path == body.path)
        .cloned();

    let base_fp = base_file.as_ref().map(|f| content_id(f));

    let mut others = open_manifests_concurrently(&state, appid, &body.others, None);

    let path = body.path.as_str();
    let mut statuses = Vec::new();
    while let Some((r, result)) = others.next().await {
        let status = match result {
            Ok(snap) => {
                let other_fp = snap
                    .manifest()
                    .files
                    .iter()
                    .find(|f| f.path == path)
                    .map(content_id);
                match (&base_fp, &other_fp) {
                    (Some(b), Some(o)) if b == o => FileDiffStatus::Same,
                    (None, None) => FileDiffStatus::Same,
                    (_, None) => FileDiffStatus::Missing,
                    (None, Some(_)) => FileDiffStatus::Different,
                    (Some(_), Some(_)) => FileDiffStatus::Different,
                }
            }
            Err(err) => {
                tracing::warn!(
                    depot_id = %r.depot_id,
                    manifest_id = %r.manifest_id,
                    %err,
                    "open_manifest failed in file diff"
                );
                // Treat a fetch error as "different" so the candidate still surfaces for investigation.
                FileDiffStatus::Different
            }
        };
        statuses.push(FileDiffTargetStatus {
            depot_id: r.depot_id,
            manifest_id: r.manifest_id,
            status,
        });
    }

    Ok(Json(FileDiffTargetsResponse { statuses }))
}

#[derive(Deserialize, ToSchema)]
pub struct ManifestDiffTargetsRequest {
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
    /// Whitespace-separated AND-tokens, lowercased substring match on path; empty means every path counts.
    #[serde(default)]
    pub query: String,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "matching_targets": [
        {"depot_id": 1234567, "manifest_id": "9876543210987654321"},
        {"depot_id": 1234567, "manifest_id": "5555666677778888999"}
    ]
}))]
pub struct ManifestDiffTargetsResponse {
    /// Targets with a path matching `query` that differs from base or is absent there; targets with no such difference are omitted.
    pub matching_targets: Vec<ManifestRefShort>,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestRefShort {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
}

/// Find manifests with changes in a path subset
///
/// Like `/manifests/diff` but answers yes/no per target instead of returning the path list. A target also counts when it has query-matching paths no `base` has (the `Removed` rows there).
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/diff-targets",
    tag = "diff",
    request_body = ManifestDiffTargetsRequest,
    responses((status = 200, body = ManifestDiffTargetsResponse))
)]
#[tracing::instrument(skip_all, fields(others = body.others.len(), query = %body.query))]
pub async fn manifest_diff_targets(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ManifestDiffTargetsRequest>,
) -> Result<Json<ManifestDiffTargetsResponse>> {
    state.steam()?; // 401 if not logged in
    let base_snap = state
        .open_manifest(
            appid,
            body.base.depot_id,
            body.base.manifest_id,
            &body.base.branch,
        )
        .await?;

    // Path subset: lowercased AND-token substring match on path; no tokens means every base path passes.
    let tokens: Vec<String> = body
        .query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .collect();
    let base = base_snap.manifest();
    let mut base_subset: HashMap<&str, &DepotFile> = HashMap::with_capacity(base.files.len());
    let mut base_paths: HashSet<&str> = HashSet::with_capacity(base.files.len());
    for f in &base.files {
        if f.is_dir() {
            continue;
        }
        base_paths.insert(f.path.as_str());
        if !tokens.is_empty() {
            let path_lc = f.path.to_lowercase();
            if !tokens.iter().all(|t| path_lc.contains(t)) {
                continue;
            }
        }
        base_subset.insert(f.path.as_str(), f);
    }

    let mut others = open_manifests_concurrently(
        &state,
        appid,
        &body.others,
        Some((body.base.depot_id, body.base.manifest_id)),
    );

    let mut matching_targets: Vec<ManifestRefShort> = Vec::new();
    while let Some((r, result)) = others.next().await {
        let snap = match result {
            Ok(s) => s,
            Err(err) => {
                // Same policy as `file_diff_targets`: treat an unfetchable target as different so it still surfaces.
                tracing::warn!(
                    depot_id = %r.depot_id,
                    manifest_id = %r.manifest_id,
                    %err,
                    "open_manifest failed in manifest_diff_targets"
                );
                matching_targets.push(ManifestRefShort {
                    depot_id: r.depot_id,
                    manifest_id: r.manifest_id,
                });
                continue;
            }
        };
        let other = snap.manifest();
        let mut other_by_path: HashMap<&str, &DepotFile> =
            HashMap::with_capacity(other.files.len());
        for f in &other.files {
            if f.is_dir() {
                continue;
            }
            other_by_path.insert(f.path.as_str(), f);
        }
        let mut has_diff = false;
        for (path, base_f) in &base_subset {
            match other_by_path.get(path) {
                None => {
                    has_diff = true;
                    break;
                }
                Some(other_f) => {
                    if content_id(base_f) != content_id(other_f) {
                        has_diff = true;
                        break;
                    }
                }
            }
        }
        // Target-only paths count as a diff: the tree renders them as `Removed` rows.
        if !has_diff {
            for path in other_by_path.keys() {
                if base_paths.contains(*path) {
                    continue;
                }
                if !tokens.is_empty() {
                    let path_lc = path.to_lowercase();
                    if !tokens.iter().all(|t| path_lc.contains(t)) {
                        continue;
                    }
                }
                has_diff = true;
                break;
            }
        }
        if has_diff {
            matching_targets.push(ManifestRefShort {
                depot_id: r.depot_id,
                manifest_id: r.manifest_id,
            });
        }
    }

    Ok(Json(ManifestDiffTargetsResponse { matching_targets }))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct DeepDiffQuery {
    /// Branch of the base manifest. Defaults to `public`.
    #[serde(default = "super::default_branch")]
    pub branch: String,
    pub target_depot_id: DepotId,
    pub target_manifest_id: ManifestId,
    #[serde(default = "super::default_branch")]
    pub target_branch: String,
}

/// Deep manifest diff
///
/// Like [`manifest_diff`], but a changed Unity file only counts when its structured diff is non-empty (normalization noise like PPtr renumbering and HingeJoint2D m_ConnectedAnchor float noise is dropped); DLLs stay fingerprint-level since decompiling is expensive.
/// `Added`/`Removed` rows pass through as-is — no base-side content to structural-diff.
/// Downloads both sides of every Unity candidate, so this is far pricier than `manifest_diff`.
/// Served `immutable` because the response is a pure function of the two manifests — unless any per-file diff errored during the sweep (errors keep the file as `changed`, and caching that approximation would freeze it).
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/structured-diff-filter",
    tag = "diff",
    params(DeepDiffQuery),
    responses((status = 200, body = ManifestDiffResponse))
)]
#[tracing::instrument(skip_all, fields(manifest_id = %manifest_id, target_depot_id = %q.target_depot_id, target_manifest_id = %q.target_manifest_id))]
pub async fn manifest_diff_deep(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<DeepDiffQuery>,
) -> Result<Response> {
    state.steam()?; // 401 if not logged in
    let base_snap = state
        .open_manifest(appid, depot_id, manifest_id, &q.branch)
        .await?;
    let target_snap = state
        .open_manifest(
            appid,
            q.target_depot_id,
            q.target_manifest_id,
            &q.target_branch,
        )
        .await?;

    let target = target_snap.manifest();
    let mut target_by_path: HashMap<&str, &DepotFile> = HashMap::with_capacity(target.files.len());
    for f in &target.files {
        if f.is_dir() {
            continue;
        }
        target_by_path.insert(f.path.as_str(), f);
    }

    let base = base_snap.manifest();
    let mut added: Vec<String> = Vec::new();
    let mut changed_candidates: Vec<String> = Vec::new();
    for f in &base.files {
        if f.is_dir() {
            continue;
        }
        match target_by_path.get(f.path.as_str()) {
            None => added.push(f.path.clone()),
            Some(tf) if content_id(f) != content_id(tf) => changed_candidates.push(f.path.clone()),
            Some(_) => {}
        }
    }
    let base_paths: HashSet<&str> = base
        .files
        .iter()
        .filter(|f| !f.is_dir())
        .map(|f| f.path.as_str())
        .collect();
    let removed: Vec<ManifestDiffEntry> = target
        .files
        .iter()
        .filter(|f| !f.is_dir() && !base_paths.contains(f.path.as_str()))
        .map(|f| ManifestDiffEntry {
            path: f.path.clone(),
            status: ManifestDiffStatus::Removed,
            size: Some(f.size),
        })
        .collect();

    // Private env pair for the sweep, evicted at checkpoints; the shared per-manifest cache never evicts.
    #[cfg(feature = "unity")]
    let mut unity_envs = if changed_candidates.iter().any(|p| is_deep_comparable(p)) {
        Some(super::structured::build_unity_env_pair(
            Arc::new(base_snap),
            Arc::new(target_snap),
        )?)
    } else {
        None
    };
    #[cfg(not(feature = "unity"))]
    let _ = (base_snap, target_snap);

    // Chunk size is just a drain granularity; eviction below decides from the real measured cache size, not a file-size estimate.
    const CHECKPOINT_LEN: usize = 32;
    const CACHE_BYTES_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
    let started = std::time::Instant::now();
    let total_changed = changed_candidates.len();

    let mut entries: Vec<ManifestDiffEntry> = Vec::new();
    for path in added {
        entries.push(ManifestDiffEntry {
            path,
            status: ManifestDiffStatus::Added,
            size: None,
        });
    }
    tracing::info!("deep diff: sweeping {total_changed} changed files");
    let mut kept_changed = 0usize;
    let mut errored_changed = 0usize;
    let mut done_changed = 0usize;
    /// Per-file sweep outcome. An errored diff keeps the file as
    /// `changed`, but taints the response: it is no longer a pure
    /// function of the manifests, so it must not be cached
    /// immutably.
    #[cfg_attr(not(feature = "unity"), allow(dead_code))]
    enum Verdict {
        Kept(String),
        Dropped,
        Errored(String),
    }
    for chunk in changed_candidates.chunks(CHECKPOINT_LEN) {
        let sem = Arc::new(Semaphore::new(8));
        let mut fu = FuturesUnordered::new();
        for path in chunk {
            let path = path.clone();
            let sem = sem.clone();
            #[cfg(feature = "unity")]
            let unity_ctx = (
                &state,
                q.branch.clone(),
                q.target_branch.clone(),
                q.target_depot_id,
                q.target_manifest_id,
                unity_envs.clone(),
            );
            fu.push(async move {
                let _permit = sem.acquire().await.expect("semaphore not closed");
                if !is_deep_comparable(&path) {
                    return Verdict::Kept(path);
                }
                #[cfg(feature = "unity")]
                {
                    let (state, branch, target_branch, target_depot_id, target_manifest_id, envs) =
                        unity_ctx;
                    let envs = envs
                        .as_ref()
                        .expect("is_deep_comparable implies unity_envs was built");
                    return match super::structured::deep_structured_diff(
                        state,
                        appid,
                        depot_id,
                        manifest_id,
                        &branch,
                        target_depot_id,
                        target_manifest_id,
                        &target_branch,
                        &path,
                        envs,
                    )
                    .await
                    {
                        Ok(tree) if tree.has_no_changes() => Verdict::Dropped,
                        Ok(_) => Verdict::Kept(path),
                        Err(err) => {
                            tracing::warn!(%path, reason = ?err, "deep diff: structured diff failed; keeping");
                            Verdict::Errored(path)
                        }
                    };
                }
                #[cfg(not(feature = "unity"))]
                unreachable!("is_deep_comparable is always false without the unity feature")
            });
        }
        while let Some(verdict) = fu.next().await {
            match verdict {
                Verdict::Dropped => {}
                Verdict::Kept(path) => {
                    kept_changed += 1;
                    entries.push(ManifestDiffEntry {
                        path,
                        status: ManifestDiffStatus::Changed,
                        size: None,
                    });
                }
                Verdict::Errored(path) => {
                    kept_changed += 1;
                    errored_changed += 1;
                    entries.push(ManifestDiffEntry {
                        path,
                        status: ManifestDiffStatus::Changed,
                        size: None,
                    });
                }
            }
            done_changed += 1;
            tracing::info!(parent: None, "deep diff: {done_changed}/{total_changed}");
        }
        #[allow(unused_mut)]
        let mut cached_bytes = 0u64;
        #[cfg(feature = "unity")]
        if let Some(envs) = &mut unity_envs {
            cached_bytes = super::structured::cached_bytes(envs);
        }
        let evicted = cached_bytes > CACHE_BYTES_LIMIT;
        if evicted {
            #[cfg(feature = "unity")]
            if let Some(envs) = &mut unity_envs {
                super::structured::evict_cache(envs);
            }
        }
        tracing::debug!(cached_bytes, evicted, "deep diff: env cache checkpoint");
    }
    entries.extend(removed);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    tracing::info!(
        errored_changed,
        elapsed = ?started.elapsed(),
        "deep diff: done, {kept_changed}/{total_changed} kept"
    );
    let body = Json(ManifestDiffResponse { entries });
    Ok(if errored_changed == 0 {
        (ImmutableCache, body).into_response()
    } else {
        (CacheSeconds(0), body).into_response()
    })
}

#[derive(PartialEq, Eq)]
pub enum ContentId<'a> {
    File(FileHash),
    Symlink(&'a str),
    Directory,
}

pub fn content_id(f: &DepotFile) -> ContentId<'_> {
    match &f.kind {
        DepotFileKind::File { sha, .. } => ContentId::File(*sha),
        DepotFileKind::Symlink { target } => ContentId::Symlink(target),
        DepotFileKind::Directory => ContentId::Directory,
    }
}

/// Opens every distinct `(depot_id, manifest_id)` in `refs`, deduped, `exclude` skipped, bounded by an 8-wide semaphore. Yields each ref alongside its result.
pub(crate) fn open_manifests_concurrently<'a>(
    state: &'a AppState,
    appid: AppId,
    refs: &'a [ManifestRef],
    exclude: Option<(DepotId, ManifestId)>,
) -> FuturesUnordered<impl Future<Output = (ManifestRef, Result<Snapshot, VfsError>)> + 'a> {
    let sem = Arc::new(Semaphore::new(8));
    let fu = FuturesUnordered::new();
    let mut seen = HashSet::new();
    for r in refs {
        if !seen.insert((r.depot_id, r.manifest_id)) {
            continue;
        }
        if exclude == Some((r.depot_id, r.manifest_id)) {
            continue;
        }
        let sem = sem.clone();
        let r = r.clone();
        fu.push(async move {
            let _permit = sem.acquire().await.expect("semaphore not closed");
            let result = state
                .open_manifest(appid, r.depot_id, r.manifest_id, &r.branch)
                .await;
            (r, result)
        });
    }
    fu
}

/// Which file types the deep filter structural-diffs — see
/// [`super::structured::is_deep_comparable`] for the definition.
pub use super::structured::is_deep_comparable;

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms `Arc::get_mut` sees refcount 1 once a `FuturesUnordered` fully drains.
    #[tokio::test]
    async fn arc_get_mut_succeeds_after_drain() {
        let mut shared = Arc::new(0i32);
        for _ in 0..5 {
            let mut fu = FuturesUnordered::new();
            for _ in 0..8 {
                let clone = shared.clone();
                fu.push(async move {
                    tokio::task::spawn_blocking(move || {
                        let _keep_alive = clone;
                    })
                    .await
                    .unwrap();
                });
            }
            while (fu.next().await).is_some() {}
            assert!(Arc::get_mut(&mut shared).is_some());
        }
    }
}
