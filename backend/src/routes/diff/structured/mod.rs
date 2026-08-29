// TODO(ai-review): review for style and correctness
//! Structured (per-object) diff tree for one file across two manifests.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use tracing::Instrument as _;
use transform::structured::StructuredTree;

use crate::http::ApiError;
#[cfg(feature = "unity")]
use crate::state::manifest_cache::Environment;
use crate::state::{AppState, Snapshot};
use crate::steam::{AppId, DepotId, ManifestId};

pub mod node;

use super::default_branch;
use crate::routes::Result;

/// Query string for the structured-diff endpoint. Extends
/// [`FileViewQuery`](crate::routes::files::FileViewQuery) with the target side as flat params, keeping the call GETtable so the immutable Cache-Control works (POST would block it).
#[derive(Deserialize, utoipa::IntoParams)]
pub struct StructuredDiffQuery {
    pub path: String,
    /// Branch of the base manifest. Defaults to `public`.
    #[serde(default = "default_branch")]
    pub branch: String,
    pub target_depot_id: DepotId,
    pub target_manifest_id: ManifestId,
    #[serde(default = "default_branch")]
    pub target_branch: String,
}

/// Structured tree diff
///
/// Returns a pruned tree tagged `added`/`removed`/`changed`/`unchanged` per node; unchanged leaves are dropped. Per-node content is fetched separately, lazily, via `/file/structured/node`, once per side.
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured-diff",
    tag = "diff",
    params(StructuredDiffQuery),
    responses(
        (status = 200, body = transform::structured::StructuredTree),
        (status = 415, description = "No structured-diff builder for this file type")
    )
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_structured_diff(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<StructuredDiffQuery>,
) -> Result<(crate::http::ImmutableCache, Json<StructuredTree>)> {
    state.steam()?; // 401 if not logged in
    match build_structured_diff_tree(
        &state,
        appid,
        depot_id,
        manifest_id,
        &q.branch,
        q.target_depot_id,
        q.target_manifest_id,
        &q.target_branch,
        &q.path,
    )
    .await?
    {
        Some(tree) => Ok((crate::http::ImmutableCache, Json(tree))),
        None => Err(ApiError::unsupported_media_type(format!(
            "structured diff not supported for: {}",
            q.path
        ))),
    }
}

/// Build the structured-diff tree for one file across two manifests, or `None` if the file type has no structured-diff builder. Downloads both sides' chunks and runs the per-format differ.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(path = %path))]
pub(super) async fn build_structured_diff_tree(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    target_depot_id: DepotId,
    target_manifest_id: ManifestId,
    target_branch: &str,
    path: &str,
) -> Result<Option<StructuredTree>> {
    use transform::Transformer;

    let kind = transform::tools::transformer_for(path);

    let (base, target) = tokio::try_join!(
        prepare_structured_side(state, appid, depot_id, manifest_id, path, branch),
        prepare_structured_side(
            state,
            appid,
            target_depot_id,
            target_manifest_id,
            path,
            target_branch,
        ),
    )?;

    let diff = match kind {
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized) => {
            build_unity_serialized_diff(
                state,
                appid,
                depot_id,
                manifest_id,
                branch,
                base,
                target_depot_id,
                target_manifest_id,
                target_branch,
                target,
                path,
            )
            .await?
        }
        #[cfg(feature = "unity")]
        Some(Transformer::UnityBundle) => {
            build_unity_bundle_diff(
                state,
                appid,
                depot_id,
                manifest_id,
                branch,
                base,
                target_depot_id,
                target_manifest_id,
                target_branch,
                target,
                path,
            )
            .await?
        }
        Some(Transformer::Dll) => build_dll_diff(state, &base, &target, path).await?,
        _ => return Ok(None),
    };

    Ok(Some(diff))
}

/// Build the structured diff for a Unity SerializedFile.
#[cfg(feature = "unity")]
#[allow(clippy::too_many_arguments)]
async fn build_unity_serialized_diff(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    base: Arc<Snapshot>,
    target_depot_id: DepotId,
    target_manifest_id: ManifestId,
    target_branch: &str,
    target: Arc<Snapshot>,
    path: &str,
) -> Result<StructuredTree> {
    let path = path.to_owned();
    let (base_env, base_data_dir) = unity_env(state, appid, depot_id, manifest_id, branch, base)?;
    let (target_env, target_data_dir) = unity_env(
        state,
        appid,
        target_depot_id,
        target_manifest_id,
        target_branch,
        target,
    )?;
    tokio::task::spawn_blocking(move || {
        transform::unity::serializedfile::diff::build_diff(
            &base_env,
            &base_data_dir,
            &target_env,
            &target_data_dir,
            &path,
        )
    })
    .instrument(tracing::info_span!("diff_build", kind = "unity_serialized"))
    .await
    .map_err(|e| ApiError::internal(format!("structured-diff task panicked: {e}")))?
    .map_err(|e| ApiError::internal(e.to_string()))
}

/// Build the structured diff for a Unity asset bundle.
#[cfg(feature = "unity")]
#[allow(clippy::too_many_arguments)]
async fn build_unity_bundle_diff(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    base: Arc<Snapshot>,
    target_depot_id: DepotId,
    target_manifest_id: ManifestId,
    target_branch: &str,
    target: Arc<Snapshot>,
    path: &str,
) -> Result<StructuredTree> {
    use rabex_env::resolver::EnvResolver;
    let path = path.to_owned();
    let (base_env, base_data_dir) = unity_env(state, appid, depot_id, manifest_id, branch, base)?;
    let (target_env, target_data_dir) = unity_env(
        state,
        appid,
        target_depot_id,
        target_manifest_id,
        target_branch,
        target,
    )?;
    tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let base_relative = path
            .strip_prefix(&format!("{base_data_dir}/"))
            .unwrap_or(&path);
        let target_relative = path
            .strip_prefix(&format!("{target_data_dir}/"))
            .unwrap_or(&path);
        let base_bytes = base_env
            .game_files
            .read_path(std::path::Path::new(base_relative))?;
        let target_bytes = target_env
            .game_files
            .read_path(std::path::Path::new(target_relative))?;
        transform::unity::bundle::build_diff(
            &base_env,
            base_bytes,
            &target_env,
            target_bytes,
            &path,
        )
    })
    .instrument(tracing::info_span!("diff_build", kind = "unity_bundle"))
    .await
    .map_err(|e| ApiError::internal(format!("structured-diff task panicked: {e}")))?
    .map_err(|e| ApiError::internal(e.to_string()))
}

/// Build the structured diff for a .NET assembly; also warms the full-decompile cache.
async fn build_dll_diff(
    state: &AppState,
    base: &Arc<Snapshot>,
    target: &Arc<Snapshot>,
    path: &str,
) -> Result<StructuredTree> {
    let cfg = state.config.load();
    let store_root = cfg.store_root.clone();
    // GNU dll-diff convention: only-in-`to` → Added; to=base, from=target, so Added means "in the viewed manifest".
    let (to_bytes, to_sha) = dll_side_bytes(base, path).await?;
    let (from_bytes, from_sha) = dll_side_bytes(target, path).await?;
    let tree = transform::dll::diff::build_tree(
        &store_root,
        &from_sha,
        &from_bytes,
        &to_sha,
        &to_bytes,
        path,
    )
    .instrument(tracing::info_span!("diff_build", kind = "dll"))
    .await
    .map_err(ApiError::from_transform)?;
    // Kick off background full decompile (idempotent, keyed by sha) so later per-type lookups hit cache.
    transform::dll::warm_full_decompile(&store_root, from_sha, from_bytes);
    transform::dll::warm_full_decompile(&store_root, to_sha, to_bytes);
    Ok(tree)
}

/// Open a manifest, find the file, enqueue and await its chunks; returns the snapshot as an `Arc` for later blocking-task use.
#[tracing::instrument(skip_all, fields(depot_id = %depot_id, manifest_id = %manifest_id))]
async fn prepare_structured_side(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    path: &str,
    branch: &str,
) -> Result<Arc<Snapshot>> {
    let snapshot = Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, branch)
            .await?,
    );
    let chunks_for_dl: Vec<_> = {
        let file = snapshot
            .manifest()
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "file not in manifest {depot_id}/{manifest_id}: {path}"
                ))
            })?;
        file.chunks()
            .iter()
            .map(|c| (c.sha, u64::from(c.size_compressed)))
            .collect()
    };
    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;
    Ok(snapshot)
}

/// Resolve the per-manifest unity `Environment` + data_dir; 415s if the manifest isn't a unity game.
#[cfg(feature = "unity")]
pub(super) fn unity_env(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    snapshot: Arc<Snapshot>,
) -> Result<(Arc<Environment>, String), ApiError> {
    let scratch = state
        .manifest_cache
        .scratch(appid, depot_id, manifest_id, branch);
    let unity = scratch
        .unity(snapshot)
        .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
    Ok((unity.env.clone(), unity.data_dir()))
}

/// Read a file's bytes + manifest-recorded sha1; the sha keys the decompile cache, so it must match the actual bytes.
async fn dll_side_bytes(snap: &Arc<Snapshot>, path: &str) -> Result<(Vec<u8>, [u8; 20])> {
    let sha = snap
        .manifest()
        .files
        .iter()
        .find(|f| f.path == path)
        .and_then(|f| f.sha())
        .ok_or_else(|| ApiError::bad_request(format!("file has no content sha: {path}")))?
        .0;
    let bytes = snap.read_full(path).await?.to_vec();
    Ok((bytes, sha))
}
