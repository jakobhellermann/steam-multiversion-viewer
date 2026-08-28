// TODO(ai-review): review for style and correctness
//! `/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured`
//! — returns a [`StructuredTree`] for files where the backend knows
//! how to build one. A companion endpoint serves lazy per-node content
//! so the initial response stays small.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use serde::Deserialize;

use crate::http::{ApiError, ImmutableCache};
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};
#[cfg(feature = "unity")]
use rabex_env::resolver::EnvResolver;
use transform::Transformer;
use transform::structured::{NodeContent, StructuredTree};

use super::Result;
use super::files::FileViewQuery;

/// Structured tree
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured",
    tag = "structured",
    params(FileViewQuery),
    responses(
        (status = 200, body = StructuredTree),
        (status = 415, description = "File has no structured representation — fall back to the plain preview / transformer")
    )
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_structured(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<(ImmutableCache, Json<StructuredTree>)> {
    state.steam()?; // 401 if not logged in
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
            .ok_or_else(|| ApiError::not_found(format!("file not in manifest: {}", q.path)))?;
        let sha = file
            .sha()
            .ok_or_else(|| ApiError::bad_request(format!("file has no content sha: {}", q.path)))?
            .0;
        (
            sha,
            file.chunks()
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
    match transform::tools::transformer_for(&path) {
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized) => {
            let scratch = state
                .manifest_cache
                .scratch(appid, depot_id, manifest_id, &q.branch);
            let unity = scratch
                .unity(snapshot.clone())
                .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
            let env = unity.env.clone();
            let data_dir = unity.data_dir();
            let tree = tokio::task::spawn_blocking(move || {
                transform::unity::serializedfile::tree::build_tree(&env, &data_dir, &path)
            })
            .await
            .map_err(|e| ApiError::internal(format!("structured-tree task panicked: {e}")))?
            .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok((ImmutableCache, Json(tree)))
        }
        #[cfg(feature = "unity")]
        Some(Transformer::UnityBundle) => {
            let scratch = state
                .manifest_cache
                .scratch(appid, depot_id, manifest_id, &q.branch);
            let unity = scratch
                .unity(snapshot.clone())
                .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
            let env = unity.env.clone();
            let data_dir = unity.data_dir();
            let tree = tokio::task::spawn_blocking(move || {
                let relative = path.strip_prefix(&format!("{data_dir}/")).unwrap_or(&path);
                let bundle_bytes = env.game_files.read_path(std::path::Path::new(relative))?;
                transform::unity::bundle::build_tree(&env, bundle_bytes, &path)
            })
            .await
            .map_err(|e| ApiError::internal(format!("structured-tree task panicked: {e}")))?
            .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok((ImmutableCache, Json(tree)))
        }
        Some(Transformer::Dll) => {
            let cfg = state.config.load();
            let bytes = snapshot.read_full(&path).await?;
            let tree = transform::dll::tree::build_tree(&cfg.store_root, &file_sha, &bytes, &path)
                .await
                .map_err(ApiError::from_transform)?;
            // Kick off the bulk `-p` decompile in the background so
            // follow-up type clicks become cache hits. Dedups per-sha
            // inside the warmer.
            transform::dll::warm_full_decompile(&cfg.store_root, file_sha, bytes.to_vec());
            Ok((ImmutableCache, Json(tree)))
        }
        _ => Err(ApiError::unsupported_media_type(format!(
            "no structured view for {path}"
        ))),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct NodeContentQuery {
    #[serde(default = "crate::routes::default_branch")]
    pub branch: String,
    pub path: String,
    /// Opaque node id from the tree's `id` field.
    pub node_id: String,
}

fn empty_node() -> (ImmutableCache, Json<NodeContent>) {
    (
        ImmutableCache,
        Json(NodeContent {
            mime: "text/plain".to_string(),
            text: String::new(),
        }),
    )
}

/// Parse the `<platform>:<blobIndex>` tail of a shader-program node id.
fn parse_program(tail: &str) -> Option<(u32, u32)> {
    let (platform, blob_index) = tail.split_once(':')?;
    Some((platform.parse().ok()?, blob_index.parse().ok()?))
}

/// Structured tree node content
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured/node",
    tag = "structured",
    params(NodeContentQuery),
    responses(
        (status = 200, body = NodeContent),
        (status = 415, description = "File has no structured representation")
    )
)]
#[tracing::instrument(skip_all, fields(path = %q.path, node_id = %q.node_id))]
pub async fn manifest_file_structured_node(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<NodeContentQuery>,
) -> Result<(ImmutableCache, Json<NodeContent>)> {
    state.steam()?; // 401 if not logged in
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
        .and_then(|f| f.sha())
        .map(|h| h.0)
        .ok_or_else(|| ApiError::not_found(format!("file not in manifest: {}", q.path)))?;

    match transform::tools::transformer_for(&q.path) {
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized) => {
            // Node ids: `obj:<pid>` (object dump), or
            // `obj:<pid>/prog:<platform>:<blobIndex>` (one shader
            // program's source). Non-object ids (sections, class-stats)
            // get an empty body so the frontend hides the panel.
            let (obj_id, prog_tail) = q
                .node_id
                .split_once("/prog:")
                .map_or((q.node_id.as_str(), None), |(o, p)| (o, Some(p)));
            let Some(path_id) =
                transform::unity::serializedfile::tree::parse_object_node_id(obj_id)
            else {
                return Ok(empty_node());
            };
            let program = match prog_tail {
                None => None,
                Some(tail) => match parse_program(tail) {
                    Some(parsed) => Some(parsed),
                    None => return Ok(empty_node()),
                },
            };
            let path = q.path.clone();
            let scratch = state
                .manifest_cache
                .scratch(appid, depot_id, manifest_id, &q.branch);
            let unity = scratch
                .unity(snapshot.clone())
                .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
            let data_dir = unity.data_dir();
            let scratch = scratch.clone();
            let (mime, text) = tokio::task::spawn_blocking(move || {
                // Re-borrow inside the blocking task so the
                // SecurePlayerPrefs key lookup (lazy I/O on the manifest's
                // Managed/Assembly-CSharp.dll) doesn't sit on the async
                // runtime.
                let unity = scratch
                    .unity_already_initialized()
                    .expect("unity scratch was initialised on the async side");
                use transform::unity::serializedfile::dump_value;
                match program {
                    Some((platform, blob_index)) => dump_value::dump_shader_program(
                        &unity.env, &data_dir, &path, path_id, platform, blob_index,
                    ),
                    None => {
                        let opts = dump_value::DumpOptions {
                            spp_key: unity.secure_player_prefs_key(),
                            playmaker_game: Some(unity),
                        };
                        dump_value::dump_object_json(&unity.env, &data_dir, &path, path_id, opts)
                    }
                }
            })
            .await
            .map_err(|e| ApiError::internal(format!("structured-node task panicked: {e}")))?
            .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok((
                ImmutableCache,
                Json(NodeContent {
                    mime: mime.to_string(),
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
                transform::unity::bundle::parse_archive_id(&q.node_id)
            else {
                return Ok(empty_node());
            };
            // `inner`: `obj:<pid>` (object dump) or
            // `obj:<pid>/prog:<platform>:<blobIndex>` (one program).
            let (obj_id, prog_tail) = inner
                .split_once("/prog:")
                .map_or((inner, None), |(o, p)| (o, Some(p)));
            let Some(path_id) =
                transform::unity::serializedfile::tree::parse_object_node_id(obj_id)
            else {
                return Ok(empty_node());
            };
            let program = match prog_tail {
                None => None,
                Some(tail) => match parse_program(tail) {
                    Some(parsed) => Some(parsed),
                    None => return Ok(empty_node()),
                },
            };
            let bundle_path = q.path.clone();
            let archive_entry = archive_entry.to_string();
            let scratch = state
                .manifest_cache
                .scratch(appid, depot_id, manifest_id, &q.branch);
            let unity = scratch
                .unity(snapshot.clone())
                .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
            let data_dir = unity.data_dir();
            let scratch = scratch.clone();
            let (mime, text) = tokio::task::spawn_blocking(move || {
                let unity = scratch
                    .unity_already_initialized()
                    .expect("unity scratch was initialised on the async side");
                let env = &unity.env;
                let relative = bundle_path
                    .strip_prefix(&format!("{data_dir}/"))
                    .unwrap_or(&bundle_path);
                let bundle_bytes = env.game_files.read_path(std::path::Path::new(relative))?;
                use transform::unity::serializedfile::dump_value;
                match program {
                    Some((platform, blob_index)) => dump_value::dump_bundle_shader_program(
                        env,
                        bundle_bytes,
                        &archive_entry,
                        path_id,
                        platform,
                        blob_index,
                    ),
                    None => {
                        let opts = dump_value::DumpOptions {
                            spp_key: unity.secure_player_prefs_key(),
                            playmaker_game: Some(unity),
                        };
                        dump_value::dump_bundle_object_json(
                            env,
                            &data_dir,
                            bundle_bytes,
                            &archive_entry,
                            path_id,
                            opts,
                        )
                    }
                }
            })
            .await
            .map_err(|e| ApiError::internal(format!("structured-node task panicked: {e}")))?
            .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok((
                ImmutableCache,
                Json(NodeContent {
                    mime: mime.to_string(),
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
                transform::dll::decompile_type(&cfg.store_root, &file_sha, &bytes, type_name)
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok((
                ImmutableCache,
                Json(NodeContent {
                    mime: "text/x-csharp".to_string(),
                    text,
                }),
            ))
        }
        _ => Err(ApiError::unsupported_media_type(
            "no structured view for this file",
        )),
    }
}

/// Structured tree node rendered as an image (Texture2D → PNG)
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured/node/image",
    tag = "structured",
    params(NodeContentQuery),
    responses(
        (status = 200, description = "PNG image of the texture", content_type = "image/png"),
        (status = 415, description = "Node is not a renderable texture")
    )
)]
#[tracing::instrument(skip_all, fields(path = %q.path, node_id = %q.node_id))]
pub async fn manifest_file_structured_node_image(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<NodeContentQuery>,
) -> Result<Response> {
    state.steam()?; // 401 if not logged in
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    match transform::tools::transformer_for(&q.path) {
        #[cfg(feature = "unity")]
        Some(Transformer::UnityBundle) => {
            let Some((archive_entry, inner)) =
                transform::unity::bundle::parse_archive_id(&q.node_id)
            else {
                return Err(ApiError::unsupported_media_type("not an object node"));
            };
            let Some(path_id) = transform::unity::serializedfile::tree::parse_object_node_id(inner)
            else {
                return Err(ApiError::unsupported_media_type("not an object node"));
            };
            let bundle_path = q.path.clone();
            let archive_entry = archive_entry.to_string();
            let scratch = state
                .manifest_cache
                .scratch(appid, depot_id, manifest_id, &q.branch);
            let unity = scratch
                .unity(snapshot.clone())
                .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
            let data_dir = unity.data_dir();
            let scratch = scratch.clone();
            let png = tokio::task::spawn_blocking(move || {
                let unity = scratch
                    .unity_already_initialized()
                    .expect("unity scratch was initialised on the async side");
                let env = &unity.env;
                let relative = bundle_path
                    .strip_prefix(&format!("{data_dir}/"))
                    .unwrap_or(&bundle_path);
                let bundle_bytes = env.game_files.read_path(std::path::Path::new(relative))?;
                transform::unity::serializedfile::texture::render_bundle_texture_png(
                    env,
                    bundle_bytes,
                    &archive_entry,
                    path_id,
                )
            })
            .await
            .map_err(|e| ApiError::internal(format!("texture task panicked: {e}")))?
            .map_err(|e| ApiError::internal(e.to_string()))?;

            Ok((ImmutableCache, [(header::CONTENT_TYPE, "image/png")], png).into_response())
        }
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized) => {
            let Some(path_id) =
                transform::unity::serializedfile::tree::parse_object_node_id(&q.node_id)
            else {
                return Err(ApiError::unsupported_media_type("not an object node"));
            };
            let path = q.path.clone();
            let scratch = state
                .manifest_cache
                .scratch(appid, depot_id, manifest_id, &q.branch);
            let unity = scratch
                .unity(snapshot.clone())
                .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
            let data_dir = unity.data_dir();
            let scratch = scratch.clone();
            let png = tokio::task::spawn_blocking(move || {
                let unity = scratch
                    .unity_already_initialized()
                    .expect("unity scratch was initialised on the async side");
                transform::unity::serializedfile::texture::render_serialized_texture_png(
                    &unity.env, &data_dir, &path, path_id,
                )
            })
            .await
            .map_err(|e| ApiError::internal(format!("texture task panicked: {e}")))?
            .map_err(|e| ApiError::internal(e.to_string()))?;

            Ok((ImmutableCache, [(header::CONTENT_TYPE, "image/png")], png).into_response())
        }
        _ => Err(ApiError::unsupported_media_type(
            "no texture preview for this file",
        )),
    }
}
