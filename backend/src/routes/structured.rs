// TODO(ai-review): review for style and correctness
//! `/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured`
//! — returns a [`StructuredTree`] for files where the backend knows
//! how to build one (today only unity serialized files via
//! [`crate::unity::tree::build_tree`]). A companion endpoint serves
//! lazy per-node content so the initial response stays small.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};
use crate::structured::{NodeContent, StructuredTree};

use super::{FileViewQuery, Result};

/// Structured tree for the file at `q.path` in the given manifest.
/// Returns 415 (Unsupported Media Type) when the file has no
/// structured representation today — callers should fall back to the
/// plain text preview / transformer in that case.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured",
    params(FileViewQuery)
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_structured(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<Json<StructuredTree>> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    // Pre-download the file's own chunks so the rabex tree-walk
    // doesn't stall mid-parse waiting on the CDN. External files
    // (typetree, shared assets) are still fetched lazily by rabex's
    // resolver when it needs them.
    let chunks_for_dl = {
        let file = snapshot
            .manifest()
            .files
            .iter()
            .find(|f| f.path == q.path)
            .ok_or_else(|| ApiError {
                status: axum::http::StatusCode::NOT_FOUND,
                message: format!("file not in manifest: {}", q.path),
            })?;
        file.chunks
            .iter()
            .map(|c| (c.sha, u64::from(c.size_compressed)))
            .collect::<Vec<_>>()
    };
    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;

    let path = q.path.clone();

    #[cfg(feature = "unity")]
    {
        if has_unity_dispatch(&path) {
            let snapshot_for_blocking = snapshot.clone();
            let tree = tokio::task::spawn_blocking(move || {
                crate::unity::tree::build_tree(snapshot_for_blocking, &path)
            })
            .await
            .map_err(|e| ApiError {
                status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("structured-tree task panicked: {e}"),
            })?
            .map_err(|e| ApiError {
                status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                message: e.to_string(),
            })?;
            return Ok(Json(tree));
        }
    }

    Err(ApiError {
        status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        message: format!("no structured view for {path}"),
    })
}

/// Lazy per-node content. V1 returns a placeholder; once we wire the
/// component-deserialiser this will hand back a JSON dump of the
/// component's typetree-decoded values.
#[derive(Deserialize, ToSchema)]
pub struct NodeContentRequest {
    pub node_id: String,
}

#[utoipa::path(
    post,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured/node",
    params(FileViewQuery),
    request_body = NodeContentRequest,
)]
#[tracing::instrument(skip_all, fields(path = %q.path, node_id = %body.node_id))]
pub async fn manifest_file_structured_node(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
    Json(body): Json<NodeContentRequest>,
) -> Result<Json<NodeContent>> {
    #[cfg(feature = "unity")]
    {
        if has_unity_dispatch(&q.path) {
            // Resolve the node id to a path-id; non-object ids (section
            // headers, class-stats rows) get a friendly placeholder
            // rather than a 400.
            let Some(path_id) = crate::unity::tree::parse_object_node_id(&body.node_id) else {
                // Section / class-stats nodes — empty body, frontend
                // hides the panel.
                return Ok(Json(NodeContent {
                    mime: "text/plain".to_string(),
                    text: String::new(),
                }));
            };
            let snapshot = Arc::new(
                state
                    .open_manifest(appid, depot_id, manifest_id, &q.branch)
                    .await?,
            );
            let path = q.path.clone();
            let text = tokio::task::spawn_blocking(move || {
                crate::unity::tree::dump_object_json(snapshot, &path, path_id)
            })
            .await
            .map_err(|e| ApiError {
                status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("structured-node task panicked: {e}"),
            })?
            .map_err(|e| ApiError {
                status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                message: e.to_string(),
            })?;
            return Ok(Json(NodeContent {
                mime: "application/json".to_string(),
                text,
            }));
        }
    }

    let _ = (state, appid, depot_id, manifest_id, q, body);
    Err(ApiError {
        status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        message: "no structured view for this file".to_string(),
    })
}

/// Today's only dispatch path. Mirrors the heuristic used by
/// `transform::tools::transformer_for` so a file that gets a unity
/// transformer also gets a unity tree.
#[cfg(feature = "unity")]
fn has_unity_dispatch(path: &str) -> bool {
    use crate::transform::Transformer;
    matches!(
        crate::transform::tools::transformer_for(path),
        Some(Transformer::UnitySerialized)
    )
}
