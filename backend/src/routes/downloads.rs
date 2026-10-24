// TODO(ai-review): review for style and correctness
//! Routes for driving and observing the background chunk-download
//! manager. Split out from `routes/mod.rs` because the cluster has its
//! own concerns (cancel semantics) that don't share much
//! with the read-only manifest/file routes.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use steam_depot_vfs::ChunkHash;
use utoipa::ToSchema;

use crate::http::ApiError;
use crate::state::AppState;
use crate::state::downloads::{DownloadStats, EnqueueSummary};
use crate::steam::{AppId, DepotId, ManifestId};

use super::{Result, default_branch};

#[derive(Debug, Deserialize, ToSchema)]
pub struct DownloadManifestBody {
    #[serde(default = "default_branch")]
    pub branch: String,
    /// If absent, download all chunks of the manifest. Otherwise restrict to
    /// chunks referenced by the listed file paths.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

/// Enqueue manifest download
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/download",
    tag = "downloads",
    request_body = DownloadManifestBody,
    responses((status = 200, body = EnqueueSummary))
)]
#[tracing::instrument(skip_all)]
pub async fn manifest_download(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Json(body): Json<DownloadManifestBody>,
) -> Result<Json<EnqueueSummary>> {
    state.steam().await?; // 401 if not logged in
    let snapshot = state
        .open_manifest(appid, depot_id, manifest_id, &body.branch)
        .await?;
    let manifest = snapshot.manifest();

    let mut chunks: Vec<(ChunkHash, u64)> = Vec::new();
    let mut seen = HashSet::new();
    if let Some(ref paths) = body.paths {
        let filter: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let mut matched: HashSet<&str> = HashSet::new();
        for f in &manifest.files {
            if !filter.contains(f.path.as_str()) {
                continue;
            }
            matched.insert(f.path.as_str());
            for c in f.chunks() {
                if seen.insert(c.sha) {
                    chunks.push((c.sha, u64::from(c.size_compressed)));
                }
            }
        }
        let unknown: Vec<&str> = filter
            .iter()
            .filter(|p| !matched.contains(*p))
            .copied()
            .collect();
        if !unknown.is_empty() {
            return Err(ApiError::bad_request(format!(
                "unknown paths in manifest: {unknown:?}"
            )));
        }
    } else {
        for f in &manifest.files {
            for c in f.chunks() {
                if seen.insert(c.sha) {
                    chunks.push((c.sha, u64::from(c.size_compressed)));
                }
            }
        }
    }

    let summary = state.downloads.enqueue(Arc::new(snapshot), chunks);
    tracing::info!(
        %depot_id, %manifest_id, branch = %body.branch,
        enqueued = summary.enqueued_chunks,
        skipped_present = summary.already_present_chunks,
        bytes = summary.enqueued_bytes,
        "download enqueued"
    );
    Ok(Json(summary))
}

/// Active download progress
#[utoipa::path(
    get,
    path = "/api/downloads",
    tag = "downloads",
    responses((status = 200, body = DownloadStats))
)]
pub async fn downloads_snapshot(State(state): State<AppState>) -> Json<DownloadStats> {
    Json(state.downloads.current())
}

/// Cancel all downloads
#[utoipa::path(
    post,
    path = "/api/downloads/cancel",
    tag = "downloads",
    responses((status = 200, body = DownloadStats))
)]
#[tracing::instrument(skip_all)]
pub async fn downloads_cancel(State(state): State<AppState>) -> Json<DownloadStats> {
    state.downloads.cancel();
    Json(state.downloads.current())
}
