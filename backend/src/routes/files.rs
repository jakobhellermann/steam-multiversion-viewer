// TODO(ai-review): review for style and correctness
//! Per-file endpoints: metadata + inline preview, raw bytes,
//! and text-rendered through a registered transformer.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::json;
use utoipa::{IntoParams, ToSchema};

use crate::http::{ApiError, ImmutableCache};
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

use super::library::ManifestFileKind;
use super::{Result, default_branch};

#[derive(Debug, Deserialize, IntoParams)]
pub struct FileViewQuery {
    #[serde(default = "default_branch")]
    pub branch: String,
    pub path: String,
    /// When false, return metadata only (size, sha, kind,
    /// chunks_present) — skip the chunk download and inline-content
    /// read.
    #[serde(default = "default_true")]
    pub auto_fetch: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "path": "Data/example/asset.bin",
    "size": 62451264,
    "kind": "file",
    "chunk_count": 61,
    "chunks_present": 61,
    "linktarget": null,
    "sha": "0000000000000000000000000000000000000000",
    "content_kind": "unknown",
    "content": null,
    "preview_cap_bytes": 67108864,
    "transformer": null,
    "structured": null
}))]
pub struct FileView {
    pub path: String,
    pub size: u64,
    pub kind: ManifestFileKind,
    pub chunk_count: u32,
    pub chunks_present: u32,
    pub linktarget: Option<String>,
    /// Steam-side content sha1 of the file, hex-encoded. Stable identity
    /// for "is this the same file?" comparisons across manifests. None
    /// when the manifest doesn't carry one (e.g. symlinks).
    pub sha: Option<String>,
    /// SHA-256-ish guess: "text" | "binary" | "unknown" (kind != File).
    pub content_kind: FileContentKind,
    /// Inline UTF-8 content for small text files. `None` means "too large
    /// to preview" or "not a text-like extension". The caller decides
    /// whether to offer a fetch/download instead.
    pub content: Option<String>,
    /// When `content` is `None` because of a size cap, the cap that was
    /// applied (so the caller can surface "this file is X MiB, cap is
    /// Y MiB" without hardcoding the limit).
    pub preview_cap_bytes: u64,
    /// Set when a text transformer is registered for this file's
    /// extension — callers should treat it as "the `/file/transformed`
    /// endpoint will yield text for this file" rather than guessing
    /// from the extension themselves.
    pub transformer: Option<TransformerInfo>,
    /// Set when the backend can build a [`transform::structured::StructuredTree`]
    /// for this file. The frontend uses it to decide whether to render
    /// the tree view instead of (or alongside) the plain preview.
    pub structured: Option<StructuredInfo>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct TransformerInfo {
    /// MIME type of the transformer's output (e.g. `text/x-csharp`).
    pub mime: String,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct StructuredInfo {
    /// Renderer hint matching `StructuredTree::kind` (e.g.
    /// `"unity-serialized"`).
    pub kind: String,
}

#[derive(Serialize, ToSchema, Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FileContentKind {
    Text,
    Binary,
    /// Directory / symlink / anything not a regular file.
    Unknown,
    /// File is larger than the preview cap; we didn't read it, so we
    /// can't tell text from binary.
    TooLarge,
}

/// Maximum size we'll auto-fetch for inline preview. Above this we just
/// return metadata so a click on a 2 GiB asset doesn't silently warm
/// hundreds of chunks.
const PREVIEW_CAP_BYTES: u64 = 64 * 1024 * 1024;

/// Content-based text/binary heuristic: a NUL byte in the first sniff
/// window is a hard "binary" signal. Otherwise the bytes must parse as
/// UTF-8 to count as text.
fn looks_like_text(bytes: &[u8]) -> bool {
    const SNIFF: usize = 8192;
    let head = &bytes[..bytes.len().min(SNIFF)];
    if head.contains(&0) {
        return false;
    }
    std::str::from_utf8(bytes).is_ok()
}

/// File metadata and inline preview
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file",
    tag = "files",
    params(FileViewQuery),
    responses((status = 200, body = FileView))
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<Json<FileView>> {
    state.steam()?; // 401 if not logged in
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    // Extract everything we need from the borrowed manifest before
    // handing the snapshot Arc to the download manager.
    let (
        file_path,
        file_size,
        file_kind,
        file_linktarget,
        file_sha,
        chunk_count,
        chunks_for_dl,
        chunk_shas,
    ) = {
        let manifest = snapshot.manifest();
        let file = manifest
            .files
            .iter()
            .find(|f| f.path == q.path)
            .ok_or_else(|| ApiError::not_found(format!("file not in manifest: {}", q.path)))?;
        (
            file.path.clone(),
            file.size,
            ManifestFileKind::from(file.kind),
            file.linktarget.clone(),
            file.sha.map(hex_encode),
            file.chunks.len() as u32,
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
            file.chunks.iter().map(|c| c.sha).collect::<Vec<_>>(),
        )
    };

    let (content_kind, content) = if !q.auto_fetch {
        // Caller wants metadata only — skip the download + content read.
        // We don't classify text-vs-binary without reading the bytes,
        // so return `Unknown`.
        (FileContentKind::Unknown, None)
    } else {
        match file_kind {
            ManifestFileKind::File if file_size > PREVIEW_CAP_BYTES => {
                (FileContentKind::TooLarge, None)
            }
            ManifestFileKind::File => {
                // Route the fetch through the ChunkService so the live
                // drawer reflects the load and the chunks-present index is
                // kept consistent. `enqueue_and_wait` returns once every
                // chunk has either landed on disk (and been recorded in the
                // index) or failed; `read_full` after that is a pure cache
                // hit.
                state
                    .downloads
                    .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
                    .await;
                let bytes = snapshot.read_full(&file_path).await?;
                if looks_like_text(&bytes) {
                    match String::from_utf8(bytes.to_vec()) {
                        Ok(s) => (FileContentKind::Text, Some(s)),
                        Err(_) => (FileContentKind::Binary, None),
                    }
                } else {
                    (FileContentKind::Binary, None)
                }
            }
            _ => (FileContentKind::Unknown, None),
        }
    };

    let chunks_present = {
        let index = state.store_index.read().expect("store_index poisoned");
        chunk_shas.iter().filter(|sha| index.has_chunk(sha)).count() as u32
    };

    let transformer = transform::tools::transformer_for(&file_path).map(|t| TransformerInfo {
        mime: t.output_mime().to_string(),
    });
    let structured = structured_info_for(&file_path);

    Ok(Json(FileView {
        path: file_path,
        size: file_size,
        kind: file_kind,
        chunk_count,
        chunks_present,
        linktarget: file_linktarget,
        sha: file_sha,
        content_kind,
        content,
        preview_cap_bytes: PREVIEW_CAP_BYTES,
        transformer,
        structured,
    }))
}

/// Probe whether the backend has a structured-tree builder for this
/// file. Keeps the route layer out of feature-cfg territory.
fn structured_info_for(path: &str) -> Option<StructuredInfo> {
    use transform::Transformer;
    match transform::tools::transformer_for(path) {
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized | Transformer::UnityBundle) => Some(StructuredInfo {
            kind: transform::unity::serializedfile::tree::TREE_KIND.to_string(),
        }),
        Some(Transformer::Dll) => Some(StructuredInfo {
            kind: transform::dll::tree::TREE_KIND.to_string(),
        }),
        _ => None,
    }
}

pub(super) fn hex_encode(bytes: [u8; 20]) -> String {
    let mut s = String::with_capacity(40);
    for b in bytes {
        use std::fmt::Write as _;
        write!(&mut s, "{b:02x}").expect("write to String");
    }
    s
}

/// Raw file bytes
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/raw",
    tag = "files",
    params(FileViewQuery),
    responses(
        (
            status = 200,
            description = "Raw file bytes; Content-Type is sniffed from the path",
            content_type = "application/octet-stream",
            body = Vec<u8>,
        )
    )
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_raw(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<Response> {
    state.steam()?; // 401 if not logged in
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    let (file_path, file_kind, chunks_for_dl) = {
        let manifest = snapshot.manifest();
        let file = manifest
            .files
            .iter()
            .find(|f| f.path == q.path)
            .ok_or_else(|| ApiError::not_found(format!("file not in manifest: {}", q.path)))?;
        (
            file.path.clone(),
            ManifestFileKind::from(file.kind),
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
        )
    };

    if !matches!(file_kind, ManifestFileKind::File) {
        return Err(ApiError::bad_request(format!("not a file: {file_path}")));
    }

    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;
    // PERF: read_full buffers the entire file in memory before we send a
    // byte. Fine for the typical image/audio asset, but a 2 GiB bundle
    // would blow up here. Switch to a streaming reader on DepotSnapshot
    // and pipe it into an axum Body once the use case shows up.
    let bytes = snapshot.read_full(&file_path).await?;

    let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
    Ok((
        ImmutableCache,
        [(header::CONTENT_TYPE, mime.as_ref().to_string())],
        bytes.to_vec(),
    )
        .into_response())
}

/// Transformer-rendered text
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/transformed",
    tag = "files",
    params(FileViewQuery),
    responses(
        (
            status = 200,
            description = "Content-Type matches the transformer (e.g. text/x-csharp)",
            content_type = "text/plain",
            body = String,
        ),
        (status = 415, description = "No transformer registered, or the file is served through /file/structured instead (.NET, Unity bundles)")
    )
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_transformed(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
) -> Result<Response> {
    state.steam()?; // 401 if not logged in
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, &q.branch)
            .await?,
    );

    let (file_path, file_sha, chunks_for_dl) = {
        let manifest = snapshot.manifest();
        let file = manifest
            .files
            .iter()
            .find(|f| f.path == q.path)
            .ok_or_else(|| ApiError::not_found(format!("file not in manifest: {}", q.path)))?;
        let sha = file
            .sha
            .ok_or_else(|| ApiError::bad_request(format!("file has no content sha: {}", q.path)))?;
        (
            file.path.clone(),
            sha,
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
        )
    };

    let transformer = transform::tools::transformer_for(&file_path).ok_or_else(|| {
        ApiError::unsupported_media_type(format!("no transformer for {file_path}"))
    })?;

    let content_type = format!("{}; charset=utf-8", transformer.output_mime());

    let cfg = state.config.load();
    // Cache hit short-circuits the (potentially expensive) tool run.
    if let Some(cached) = transform::read_cached(&cfg.store_root, &file_sha)? {
        return Ok((
            ImmutableCache,
            [(header::CONTENT_TYPE, content_type.clone())],
            cached,
        )
            .into_response());
    }

    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;
    let text = match transformer {
        transform::Transformer::Cli(tool) => {
            let bytes = snapshot.read_full(&file_path).await?;
            transform::run_and_cache(&cfg.store_root, tool, &file_sha, &bytes)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
        }
        #[cfg(feature = "unity")]
        transform::Transformer::UnitySerialized => {
            run_unity_dump(
                &state,
                appid,
                depot_id,
                manifest_id,
                &q.branch,
                snapshot.clone(),
                file_path.clone(),
            )
            .await?
        }
        transform::Transformer::Dll => {
            return Err(ApiError::unsupported_media_type(
                ".NET assemblies are served through /file/structured, not /file/transformed",
            ));
        }
        #[cfg(feature = "unity")]
        transform::Transformer::UnityBundle => {
            return Err(ApiError::unsupported_media_type(
                "Unity bundles are served through /file/structured, not /file/transformed",
            ));
        }
    };

    Ok((ImmutableCache, [(header::CONTENT_TYPE, content_type)], text).into_response())
}

/// Run the synchronous rabex dump off the async worker — its inner
/// `block_in_place`/`block_on` plumbing makes it unsafe to call
/// directly from an async handler.
#[cfg(feature = "unity")]
async fn run_unity_dump(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    snapshot: Arc<crate::state::Snapshot>,
    path: String,
) -> Result<String, ApiError> {
    let scratch = state
        .manifest_cache
        .scratch(appid, depot_id, manifest_id, branch);
    let unity = scratch
        .unity(snapshot)
        .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
    let env = unity.env.clone();
    let data_dir = unity.data_dir();
    tokio::task::spawn_blocking(move || {
        transform::unity::dump_unity_serialized(&env, &data_dir, &path)
    })
    .await
    .map_err(|e| ApiError::internal(format!("unity dump task panicked: {e}")))?
    .map_err(|e| ApiError::internal(e.to_string()))
}
