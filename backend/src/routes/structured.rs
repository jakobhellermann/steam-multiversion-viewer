// TODO(ai-review): review for style and correctness
//! `/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured`
//! — returns a [`StructuredTree`] for files where the backend knows
//! how to build one. A companion endpoint serves lazy per-node content
//! so the initial response stays small.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use crate::error::ApiError;
use crate::http::ImmutableCache;
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};
use ::transform::Transformer;
use ::transform::structured::{NodeContent, StructuredTree};

use super::Result;
use super::files::FileViewQuery;

/// Structured tree for the file at `q.path` in the given manifest.
/// Returns 415 (Unsupported Media Type) when the file has no
/// structured representation today — callers should fall back to the
/// plain text preview / transformer in that case.
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured",
    tag = "structured",
    params(FileViewQuery)
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_structured(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<(ImmutableCache, Json<StructuredTree>)> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    // Pre-download the file's own chunks so the actual tree-building
    // step doesn't stall mid-parse waiting on the CDN. Per-format
    // external files (rabex typetrees, etc) are fetched lazily by
    // their own resolvers.
    let (file_sha, chunks_for_dl) = {
        let file = snapshot
            .manifest()
            .files
            .iter()
            .find(|f| f.path == q.path)
            .ok_or_else(|| ApiError {
                status: axum::http::StatusCode::NOT_FOUND,
                message: format!("file not in manifest: {}", q.path),
            })?;
        let sha = file.sha.ok_or_else(|| ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: format!("file has no content sha: {}", q.path),
        })?;
        (
            sha,
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
        )
    };
    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;

    let path = q.path.clone();
    match ::transform::tools::transformer_for(&path) {
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized) => {
            let snapshot_for_blocking = snapshot.clone();
            let tree = tokio::task::spawn_blocking(move || {
                ::transform::unity::serializedfile::tree::build_tree(snapshot_for_blocking, &path)
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
            Ok((ImmutableCache, Json(tree)))
        }
        #[cfg(feature = "unity")]
        Some(Transformer::UnityBundle) => {
            let snapshot_for_blocking = snapshot.clone();
            let tree = tokio::task::spawn_blocking(move || {
                ::transform::unity::bundle::build_tree(snapshot_for_blocking, &path)
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
            Ok((ImmutableCache, Json(tree)))
        }
        Some(Transformer::Dll) => {
            let cfg = state.config.load();
            let bytes = snapshot.read_full(&path).await?;
            let tree =
                ::transform::dll::tree::build_tree(&cfg.store_root, &file_sha, &bytes, &path)
                    .await
                    .map_err(|e| ApiError {
                        status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        message: e.to_string(),
                    })?;
            // Kick off the bulk `-p` decompile in the background so
            // follow-up type clicks become cache hits. Dedups per-sha
            // inside the warmer.
            ::transform::dll::warm_full_decompile(&cfg.store_root, file_sha, bytes.to_vec());
            Ok((ImmutableCache, Json(tree)))
        }
        _ => Err(ApiError {
            status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            message: format!("no structured view for {path}"),
        }),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct NodeContentQuery {
    #[serde(default = "crate::routes::default_branch")]
    pub branch: String,
    pub path: String,
    pub node_id: String,
}

/// Lazy per-node content.
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured/node",
    tag = "structured",
    params(NodeContentQuery)
)]
#[tracing::instrument(skip_all, fields(path = %q.path, node_id = %q.node_id))]
pub async fn manifest_file_structured_node(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<NodeContentQuery>,
) -> Result<(ImmutableCache, Json<NodeContent>)> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );
    let file_sha = snapshot
        .manifest()
        .files
        .iter()
        .find(|f| f.path == q.path)
        .and_then(|f| f.sha)
        .ok_or_else(|| ApiError {
            status: axum::http::StatusCode::NOT_FOUND,
            message: format!("file not in manifest: {}", q.path),
        })?;

    match ::transform::tools::transformer_for(&q.path) {
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized) => {
            // Resolve the node id to a path-id; non-object ids
            // (section headers, class-stats rows) get an empty body so
            // the frontend hides the panel.
            let Some(path_id) =
                ::transform::unity::serializedfile::tree::parse_object_node_id(&q.node_id)
            else {
                return Ok((
                    ImmutableCache,
                    Json(NodeContent {
                        mime: "text/plain".to_string(),
                        text: String::new(),
                    }),
                ));
            };
            let path = q.path.clone();
            let text = tokio::task::spawn_blocking(move || {
                ::transform::unity::serializedfile::dump_value::dump_object_json(
                    snapshot,
                    &path,
                    path_id,
                    ::transform::unity::serializedfile::dump_value::DumpSide::None,
                )
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
            Ok((
                ImmutableCache,
                Json(NodeContent {
                    mime: "application/json".to_string(),
                    text,
                }),
            ))
        }
        #[cfg(feature = "unity")]
        Some(Transformer::UnityBundle) => {
            // Bundle node ids carry an archive prefix
            // (`archive:<entry>/obj:<pid>`) so the dump can find the
            // right SerializedFile inside the container. Non-object ids
            // (archive headers, sections, raw blobs) get an empty body.
            let Some((archive_entry, inner)) =
                ::transform::unity::bundle::parse_archive_id(&q.node_id)
            else {
                return Ok((
                    ImmutableCache,
                    Json(NodeContent {
                        mime: "text/plain".to_string(),
                        text: String::new(),
                    }),
                ));
            };
            let Some(path_id) =
                ::transform::unity::serializedfile::tree::parse_object_node_id(inner)
            else {
                return Ok((
                    ImmutableCache,
                    Json(NodeContent {
                        mime: "text/plain".to_string(),
                        text: String::new(),
                    }),
                ));
            };
            let bundle_path = q.path.clone();
            let archive_entry = archive_entry.to_string();
            let text = tokio::task::spawn_blocking(move || {
                ::transform::unity::serializedfile::dump_value::dump_bundle_object_json(
                    snapshot,
                    &bundle_path,
                    &archive_entry,
                    path_id,
                    ::transform::unity::serializedfile::dump_value::DumpSide::None,
                )
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
            Ok((
                ImmutableCache,
                Json(NodeContent {
                    mime: "application/json".to_string(),
                    text,
                }),
            ))
        }
        Some(Transformer::Dll) => {
            // `type:<fully-qualified-name>` → ilspy -t. Anything else
            // (namespace nodes, file root) has no body.
            let Some(type_name) = q.node_id.strip_prefix("type:") else {
                return Ok((
                    ImmutableCache,
                    Json(NodeContent {
                        mime: "text/plain".to_string(),
                        text: String::new(),
                    }),
                ));
            };
            let cfg = state.config.load();
            let bytes = snapshot.read_full(&q.path).await?;
            let text =
                ::transform::dll::decompile_type(&cfg.store_root, &file_sha, &bytes, type_name)
                    .await
                    .map_err(|e| ApiError {
                        status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        message: e.to_string(),
                    })?;
            Ok((
                ImmutableCache,
                Json(NodeContent {
                    mime: "text/x-csharp".to_string(),
                    text,
                }),
            ))
        }
        _ => Err(ApiError {
            status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            message: "no structured view for this file".to_string(),
        }),
    }
}
