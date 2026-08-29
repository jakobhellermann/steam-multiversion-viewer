// TODO(ai-review): review for style and correctness
//! Unified text diff for one file across two manifests.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use serde::Deserialize;
use steam_vent_depot::DepotFileKind;
use utoipa::ToSchema;

use crate::http::ApiError;
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

use super::diff_label;
#[cfg(feature = "unity")]
use super::structured::unity_env;
use crate::routes::Result;
use crate::routes::files::FileViewQuery;
use crate::routes::library::ManifestRef;

#[derive(Deserialize, ToSchema)]
pub struct FileDiffRequest {
    pub path: String,
    pub target: ManifestRef,
}

/// Unified file diff
///
/// Both sides go through the same text-resolution as `/file/transformed`, using the transformer's output when one is registered.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/diff",
    tag = "diff",
    params(FileViewQuery),
    request_body = FileDiffRequest,
    responses(
        (status = 200, content_type = "text/x-diff", body = String),
        (status = 415, description = "Binary file with no registered transformer")
    )
)]
#[tracing::instrument(skip_all, fields(path = %body.path))]
pub async fn manifest_file_diff(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
    Json(body): Json<FileDiffRequest>,
) -> Result<Response> {
    state.steam()?; // 401 if not logged in
    let (base, target) = tokio::try_join!(
        resolve_diff_text(&state, appid, depot_id, manifest_id, &q.branch, &body.path),
        resolve_diff_text(
            &state,
            appid,
            body.target.depot_id,
            body.target.manifest_id,
            &body.target.branch,
            &body.path,
        ),
    )?;

    let base_label = diff_label(depot_id, manifest_id, base.creation_time);
    let target_label = diff_label(
        body.target.depot_id,
        body.target.manifest_id,
        target.creation_time,
    );
    // Diff "older to newer" so `+` always means added in the newer version; equal timestamps keep the request order.
    let (older, newer, older_label, newer_label) = if target.creation_time < base.creation_time {
        (&target, &base, target_label, base_label)
    } else {
        (&base, &target, base_label, target_label)
    };

    let diff = similar::TextDiff::from_lines(&older.text, &newer.text);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(&older_label, &newer_label)
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

struct DiffSide {
    text: String,
    /// Steam-side manifest creation timestamp (unix seconds); picks the "older" side for a chronological diff.
    creation_time: u32,
}

/// Resolve the file at `(depot_id, manifest_id, path)` to the text to diff: the transformer's output when one is registered (e.g. decompiled C# for a .dll), raw UTF-8 for plain text, or 415 for anything else binary.
async fn resolve_diff_text(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    path: &str,
) -> Result<DiffSide> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, branch)
            .await?,
    );

    let creation_time = snapshot.manifest().creation_time;
    let (file_path, file_sha, chunks_for_dl) = file_download_info(&snapshot, path)?;
    let transformer = transform::tools::transformer_for(&file_path, None);

    // These formats have no single text dump; bail before downloading anything.
    match transformer {
        Some(transform::Transformer::Dll) => {
            return Err(ApiError::unsupported_media_type(
                ".NET assemblies have no text-diff representation yet",
            ));
        }
        #[cfg(feature = "unity")]
        Some(transform::Transformer::UnityBundle) => {
            return Err(ApiError::unsupported_media_type(
                "Unity bundles have no text-diff representation yet",
            ));
        }
        _ => {}
    }

    let cfg = state.config.load();
    if transformer.is_some()
        && let Some(cached) = transform::read_cached(&cfg.store_root, &file_sha)?
    {
        return Ok(DiffSide {
            text: cached,
            creation_time,
        });
    }

    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;

    let text = match transformer {
        None => text_plain(&snapshot, &file_path).await?,
        Some(transform::Transformer::Cli(tool)) => {
            text_via_cli_tool(&snapshot, &cfg.store_root, tool, &file_sha, &file_path).await?
        }
        #[cfg(feature = "unity")]
        Some(transform::Transformer::UnitySerialized) => {
            text_via_unity_serialized(
                state,
                appid,
                depot_id,
                manifest_id,
                branch,
                snapshot.clone(),
                file_path,
            )
            .await?
        }
        Some(transform::Transformer::Dll) => unreachable!("Dll bailed above"),
        #[cfg(feature = "unity")]
        Some(transform::Transformer::UnityBundle) => unreachable!("UnityBundle bailed above"),
    };
    Ok(DiffSide {
        text,
        creation_time,
    })
}

/// Read a file's bytes as UTF-8 text; 415 if it isn't valid UTF-8.
async fn text_plain(snapshot: &crate::state::Snapshot, file_path: &str) -> Result<String> {
    let bytes = snapshot.read_full(file_path).await?;
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        ApiError::unsupported_media_type(format!(
            "file is binary and has no registered transformer: {file_path}"
        ))
    })
}

/// Run an external CLI transformer on a file's bytes and cache the result.
async fn text_via_cli_tool(
    snapshot: &crate::state::Snapshot,
    store_root: &camino::Utf8Path,
    tool: &transform::CliTool,
    file_sha: &[u8; 20],
    file_path: &str,
) -> Result<String> {
    let bytes = snapshot.read_full(file_path).await?;
    transform::run_and_cache(store_root, tool, file_sha, &bytes)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// Dump a Unity SerializedFile as text via the in-process rabex dumper.
#[cfg(feature = "unity")]
async fn text_via_unity_serialized(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    snapshot: Arc<crate::state::Snapshot>,
    file_path: String,
) -> Result<String> {
    let (env, data_dir) = unity_env(state, appid, depot_id, manifest_id, branch, snapshot)?;
    tokio::task::spawn_blocking(move || {
        transform::unity::dump_unity_serialized(&env, &data_dir, &file_path)
    })
    .await
    .map_err(|e| ApiError::internal(format!("unity dump task panicked: {e}")))?
    .map_err(|e| ApiError::internal(e.to_string()))
}

/// A file's path, content sha, and chunk (sha, compressed-size) list for download.
fn file_download_info(
    snapshot: &crate::state::Snapshot,
    path: &str,
) -> Result<(String, [u8; 20], Vec<(steam_vent_depot::ChunkHash, u64)>)> {
    let file = snapshot
        .manifest()
        .files
        .iter()
        .find(|f| f.path == path)
        .ok_or_else(|| ApiError::not_found(format!("file not in manifest: {path}")))?;
    let DepotFileKind::File { sha, chunks, .. } = &file.kind else {
        return Err(ApiError::bad_request(format!(
            "file has no content sha: {path}"
        )));
    };
    let chunks_for_dl = chunks
        .iter()
        .map(|c| (c.sha, u64::from(c.size_compressed)))
        .collect();
    Ok((file.path.clone(), sha.0, chunks_for_dl))
}
