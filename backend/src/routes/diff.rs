// TODO(ai-review): review for style and correctness
//! `POST` diff endpoints — manifest-level path diffs, per-file
//! cross-manifest status, and the unified text diff for a single file.
//!
//! All three sit under `/api/apps/{appid}/...` and read manifests via
//! [`crate::state::AppState::open_manifest`] so cache hits short-circuit
//! the network roundtrip.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
use steam_vent_depot::{DepotFile, FileKind};
use tokio::sync::Semaphore;
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

use super::{FileViewQuery, ManifestRef, Result};

#[derive(Deserialize, ToSchema)]
pub struct ManifestDiffRequest {
    /// The manifest whose paths we report. A path is included if the file
    /// under that path differs from *any* of the listed others (added,
    /// removed, or content-changed).
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestDiffResponse {
    /// Paths in `base` that differ from at least one `other`. Includes
    /// paths missing in `base` but present in some `other` ("removed").
    pub changed_paths: Vec<String>,
}

/// Symmetric path-level diff between a base manifest and a set of others.
/// Returns every path that, in at least one of the other manifests, is
/// either missing or has a different content fingerprint. Identity is
/// `(kind, size, sha, linktarget)`; absence on either side counts as a
/// difference.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/diff",
    request_body = ManifestDiffRequest
)]
#[tracing::instrument(skip_all, fields(others = body.others.len()))]
pub async fn manifest_diff(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ManifestDiffRequest>,
) -> Result<Json<ManifestDiffResponse>> {
    let base_snap = state
        .open_manifest(
            appid,
            body.base.depot_id,
            body.base.manifest_id,
            &body.base.branch,
        )
        .await?;

    // Open all `other` manifests in parallel — pulling them one by one
    // on a cold cache adds up fast when comparing across many depots.
    let sem = Arc::new(Semaphore::new(8));
    let mut fu = FuturesUnordered::new();
    let mut seen = HashSet::new();
    for r in &body.others {
        if !seen.insert((r.depot_id, r.manifest_id)) {
            continue;
        }
        if (r.depot_id, r.manifest_id) == (body.base.depot_id, body.base.manifest_id) {
            // Comparing a manifest against itself yields nothing.
            continue;
        }
        let state = &state;
        let sem = sem.clone();
        let depot_id = r.depot_id;
        let manifest_id = r.manifest_id;
        let branch = r.branch.clone();
        fu.push(async move {
            let _permit = sem.acquire().await.expect("semaphore not closed");
            state
                .open_manifest(appid, depot_id, manifest_id, &branch)
                .await
        });
    }

    let base = base_snap.manifest();
    let mut base_by_path: HashMap<&str, &DepotFile> = HashMap::with_capacity(base.files.len());
    for f in &base.files {
        if matches!(f.kind, FileKind::Directory) {
            continue;
        }
        base_by_path.insert(f.path.as_str(), f);
    }

    // Identity tuple used to decide "same content". Files without a sha
    // (e.g. symlinks) still get a stable fingerprint via the rest.
    fn fp(f: &DepotFile) -> (FileKind, u64, Option<[u8; 20]>, Option<&str>) {
        (f.kind, f.size, f.sha, f.linktarget.as_deref())
    }

    let mut changed: HashSet<String> = HashSet::new();
    while let Some(result) = fu.next().await {
        let snap = result?;
        let other = snap.manifest();
        let mut other_paths: HashSet<&str> = HashSet::with_capacity(other.files.len());
        for f in &other.files {
            if matches!(f.kind, FileKind::Directory) {
                continue;
            }
            other_paths.insert(f.path.as_str());
            match base_by_path.get(f.path.as_str()) {
                None => {
                    // Present in `other`, absent in `base` — counts as a
                    // change of `base`'s view (the file would "appear").
                    changed.insert(f.path.clone());
                }
                Some(base_f) => {
                    if fp(base_f) != fp(f) {
                        changed.insert(f.path.clone());
                    }
                }
            }
        }
        // Paths in `base` that this `other` does not have at all.
        for path in base_by_path.keys() {
            if !other_paths.contains(path) {
                changed.insert((*path).to_owned());
            }
        }
    }

    let mut changed_paths: Vec<String> = changed.into_iter().collect();
    changed_paths.sort();
    Ok(Json(ManifestDiffResponse { changed_paths }))
}

#[derive(Deserialize, ToSchema)]
pub struct FileDiffTargetsRequest {
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
    pub path: String,
}

#[derive(Serialize, ToSchema)]
pub struct FileDiffTargetsResponse {
    /// For each `other` manifest, whether the file at `path` differs
    /// from the base (`different`), doesn't exist there (`missing`), or
    /// is identical (`same`). Identity is the file's content sha (with
    /// kind/linktarget tie-breakers for non-file or symlink entries).
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

/// Per-target diff status for a single file across many manifests.
/// One round trip instead of fanning out N file-view requests — useful
/// for callers that want to filter a long candidate list down to the
/// manifests where a specific file actually changed.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/file/diff-targets",
    request_body = FileDiffTargetsRequest
)]
#[tracing::instrument(skip_all, fields(path = %body.path, others = body.others.len()))]
pub async fn file_diff_targets(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<FileDiffTargetsRequest>,
) -> Result<Json<FileDiffTargetsResponse>> {
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

    // Identity tuple — same as manifest_diff, scoped to one file.
    let base_fp = base_file
        .as_ref()
        .map(|f| (f.kind, f.size, f.sha, f.linktarget.clone()));

    let sem = Arc::new(Semaphore::new(8));
    let mut fu = FuturesUnordered::new();
    let mut seen = HashSet::new();
    for r in &body.others {
        if !seen.insert((r.depot_id, r.manifest_id)) {
            continue;
        }
        let state = &state;
        let sem = sem.clone();
        let depot_id = r.depot_id;
        let manifest_id = r.manifest_id;
        let branch = r.branch.clone();
        fu.push(async move {
            let _permit = sem.acquire().await.expect("semaphore not closed");
            let result = state
                .open_manifest(appid, depot_id, manifest_id, &branch)
                .await;
            (depot_id, manifest_id, result)
        });
    }

    let path = body.path.as_str();
    let mut statuses = Vec::new();
    while let Some((depot_id, manifest_id, result)) = fu.next().await {
        let status = match result {
            Ok(snap) => {
                let file = snap
                    .manifest()
                    .files
                    .iter()
                    .find(|f| f.path == path)
                    .cloned();
                let other_fp = file.map(|f| (f.kind, f.size, f.sha, f.linktarget));
                match (&base_fp, &other_fp) {
                    (Some(b), Some(o)) if b == o => FileDiffStatus::Same,
                    (None, None) => FileDiffStatus::Same,
                    (_, None) => FileDiffStatus::Missing,
                    (None, Some(_)) => FileDiffStatus::Different,
                    (Some(_), Some(_)) => FileDiffStatus::Different,
                }
            }
            Err(err) => {
                tracing::warn!(%depot_id, %manifest_id, %err, "open_manifest failed in file diff");
                // Treat a fetch error as "different" so the user still
                // sees the candidate — they can investigate.
                FileDiffStatus::Different
            }
        };
        statuses.push(FileDiffTargetStatus {
            depot_id,
            manifest_id,
            status,
        });
    }

    Ok(Json(FileDiffTargetsResponse { statuses }))
}

#[derive(Deserialize, ToSchema)]
pub struct FileDiffRequest {
    pub path: String,
    pub target: ManifestRef,
}

/// Unified diff between the file at `path` in the base manifest and in
/// `body.target`. Both sides go through the same text-resolution as
/// `/file/transformed` — if a transformer is registered, the diff is on
/// its output. Response body is a unified-diff text blob with
/// `Content-Type: text/x-diff`.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/diff",
    params(FileViewQuery),
    request_body = FileDiffRequest,
)]
#[tracing::instrument(skip_all, fields(path = %body.path))]
pub async fn manifest_file_diff(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
    Json(body): Json<FileDiffRequest>,
) -> Result<Response> {
    // Both sides resolve through the same helper. We deliberately run
    // the two snapshot opens sequentially — `open_manifest` is cheap
    // when cached, and serialising keeps the depot-key fetch from
    // racing with itself.
    let base =
        resolve_diff_text(&state, appid, depot_id, manifest_id, &q.branch, &body.path).await?;
    let target = resolve_diff_text(
        &state,
        appid,
        body.target.depot_id,
        body.target.manifest_id,
        &body.target.branch,
        &body.path,
    )
    .await?;

    let base_label = format!("{depot_id}/{manifest_id}");
    let target_label = format!("{}/{}", body.target.depot_id, body.target.manifest_id);
    let diff = similar::TextDiff::from_lines(&base, &target);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(&base_label, &target_label)
        .to_string();

    Ok((
        [(
            header::CONTENT_TYPE,
            "text/x-diff; charset=utf-8".to_string(),
        )],
        unified,
    )
        .into_response())
}

/// Resolve the file at `(depot_id, manifest_id, path)` to the text we
/// want to diff. Uses the transformer when one is registered (so a .dll
/// diff is between two decompiled C# blobs); falls back to the raw
/// UTF-8 bytes for plain text files; errors with 415 for anything
/// binary that has no transformer.
async fn resolve_diff_text(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    path: &str,
) -> Result<String> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, branch)
            .await?,
    );

    let (file_path, file_sha, chunks_for_dl) = {
        let manifest = snapshot.manifest();
        let file = manifest
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or_else(|| ApiError {
                status: axum::http::StatusCode::NOT_FOUND,
                message: format!("file not in manifest {depot_id}/{manifest_id}: {path}"),
            })?;
        let sha = file.sha.ok_or_else(|| ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: format!("file has no content sha: {path}"),
        })?;
        (
            file.path.clone(),
            sha,
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
        )
    };

    if let Some(transformer) = crate::transform::tools::transformer_for(&file_path) {
        let cfg = state.config.load();
        if let Some(cached) = crate::transform::read_cached(&cfg.store_root, &file_sha)? {
            return Ok(cached);
        }
        state
            .downloads
            .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
            .await;
        let bytes = snapshot.read_full(&file_path).await?;
        return crate::transform::run_and_cache(&cfg.store_root, transformer, &file_sha, &bytes)
            .await
            .map_err(|e| ApiError {
                status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                message: e.to_string(),
            });
    }

    // No transformer — only useful for files we can read as UTF-8.
    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;
    let bytes = snapshot.read_full(&file_path).await?;
    String::from_utf8(bytes.to_vec()).map_err(|_| ApiError {
        status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        message: format!("file is binary and has no registered transformer: {file_path}"),
    })
}
