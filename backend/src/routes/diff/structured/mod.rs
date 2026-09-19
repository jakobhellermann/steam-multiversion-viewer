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

/// One side of a structured comparison.
#[derive(Clone, Debug)]
pub struct StructuredDiffSide {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub branch: String,
}

/// Input for the format-agnostic structured-diff dispatcher.
#[derive(Clone, Debug)]
pub struct StructuredDiffRequest {
    pub appid: AppId,
    pub base: StructuredDiffSide,
    pub target: StructuredDiffSide,
    pub path: String,
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
    let request = StructuredDiffRequest {
        appid,
        base: StructuredDiffSide {
            depot_id,
            manifest_id,
            branch: q.branch,
        },
        target: StructuredDiffSide {
            depot_id: q.target_depot_id,
            manifest_id: q.target_manifest_id,
            branch: q.target_branch,
        },
        path: q.path,
    };
    match build_structured_diff(&state, &request).await? {
        Some(tree) => Ok((crate::http::ImmutableCache, Json(tree))),
        None => Err(ApiError::unsupported_media_type(format!(
            "structured diff not supported for: {}",
            request.path
        ))),
    }
}

/// Build a structured diff, or return `None` for unsupported file types.
#[tracing::instrument(skip_all, fields(path = %request.path))]
pub async fn build_structured_diff(
    state: &AppState,
    request: &StructuredDiffRequest,
) -> Result<Option<StructuredTree>> {
    use transform::Transformer;

    let kind = transform::tools::transformer_for(&request.path, None);

    let (base, target) = tokio::try_join!(
        prepare_structured_side(
            state,
            request.appid,
            request.base.depot_id,
            request.base.manifest_id,
            &request.path,
            &request.base.branch,
        ),
        prepare_structured_side(
            state,
            request.appid,
            request.target.depot_id,
            request.target.manifest_id,
            &request.path,
            &request.target.branch,
        ),
    )?;

    let kind = match kind {
        Some(k) => Some(k),
        None => {
            let bytes = base.read_full(&request.path).await?;
            transform::tools::transformer_for(&request.path, Some(&bytes))
        }
    };

    let diff = match kind {
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized) => {
            let (base_env, base_data_dir) = unity_env(
                state,
                request.appid,
                request.base.depot_id,
                request.base.manifest_id,
                &request.base.branch,
                base,
            )?;
            let (target_env, target_data_dir) = unity_env(
                state,
                request.appid,
                request.target.depot_id,
                request.target.manifest_id,
                &request.target.branch,
                target,
            )?;
            build_unity_serialized_diff(
                base_env,
                base_data_dir,
                target_env,
                target_data_dir,
                &request.path,
            )
            .await?
        }
        #[cfg(feature = "unity")]
        Some(Transformer::UnityBundle) => {
            let (base_env, base_data_dir) = unity_env(
                state,
                request.appid,
                request.base.depot_id,
                request.base.manifest_id,
                &request.base.branch,
                base,
            )?;
            let (target_env, target_data_dir) = unity_env(
                state,
                request.appid,
                request.target.depot_id,
                request.target.manifest_id,
                &request.target.branch,
                target,
            )?;
            build_unity_bundle_diff(
                base_env,
                base_data_dir,
                target_env,
                target_data_dir,
                &request.path,
            )
            .await?
        }
        Some(Transformer::Dll) => build_dll_diff(state, &base, &target, &request.path).await?,
        _ => return Ok(None),
    };

    Ok(Some(diff))
}

/// Build the structured diff for a Unity SerializedFile from already-resolved envs.
#[cfg(feature = "unity")]
pub(super) async fn build_unity_serialized_diff(
    base_env: Arc<Environment>,
    base_data_dir: String,
    target_env: Arc<Environment>,
    target_data_dir: String,
    path: &str,
) -> Result<StructuredTree> {
    let path = path.to_owned();
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

/// Build the structured diff for a Unity asset bundle from already-resolved envs.
#[cfg(feature = "unity")]
pub(super) async fn build_unity_bundle_diff(
    base_env: Arc<Environment>,
    base_data_dir: String,
    target_env: Arc<Environment>,
    target_data_dir: String,
    path: &str,
) -> Result<StructuredTree> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        transform::unity::bundle::build_diff(
            &base_env,
            &base_data_dir,
            &target_env,
            &target_data_dir,
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
pub(super) async fn prepare_structured_side(
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

/// Both manifests' unity envs, built once outside the shared per-manifest cache, which never evicts.
#[cfg(feature = "unity")]
#[derive(Clone)]
pub(super) struct UnityEnvPair {
    base: (Arc<Environment>, String),
    target: (Arc<Environment>, String),
}

#[cfg(feature = "unity")]
pub(super) fn build_unity_env_pair(
    base_snapshot: Arc<Snapshot>,
    target_snapshot: Arc<Snapshot>,
) -> Result<UnityEnvPair, ApiError> {
    Ok(UnityEnvPair {
        base: private_unity_env(base_snapshot)?,
        target: private_unity_env(target_snapshot)?,
    })
}

#[cfg(feature = "unity")]
fn private_unity_env(snapshot: Arc<Snapshot>) -> Result<(Arc<Environment>, String), ApiError> {
    use rabex_env::rabex::tpk::TpkTypeTreeBlob;
    use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
    use rabex_env_steam_depot_vfs::SteamDepotGameFiles;

    let game_files = SteamDepotGameFiles::new(snapshot)
        .map_err(|_| ApiError::unsupported_media_type("manifest is not a unity game"))?;
    let data_dir = game_files
        .data_dir()
        .to_str()
        .expect("data_dir is not valid UTF-8")
        .to_owned();
    let env = Arc::new(Environment::new(
        game_files,
        TypeTreeCache::new(TpkTypeTreeBlob::embedded()),
    ));
    Ok((env, data_dir))
}

/// A side still shared at this checkpoint contributes 0 rather than failing the whole measurement.
#[cfg(feature = "unity")]
pub(super) fn cached_bytes(envs: &mut UnityEnvPair) -> u64 {
    let mut total = 0;
    for (side, env) in [("base", &mut envs.base.0), ("target", &mut envs.target.0)] {
        match Arc::get_mut(env) {
            Some(env) => total += env.cached_bytes() as u64,
            None => tracing::warn!(
                side,
                "deep diff: env still shared at checkpoint, skipping cache measure"
            ),
        }
    }
    total
}

#[cfg(feature = "unity")]
pub(super) fn evict_cache(envs: &mut UnityEnvPair) {
    for (side, env) in [("base", &mut envs.base.0), ("target", &mut envs.target.0)] {
        match Arc::get_mut(env) {
            Some(env) => env.clear_cache(),
            None => tracing::warn!(
                side,
                "deep diff: env still shared at checkpoint, skipping cache evict"
            ),
        }
    }
}

#[cfg(feature = "unity")]
#[allow(clippy::too_many_arguments)]
pub(super) async fn deep_unity_diff(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    target_depot_id: DepotId,
    target_manifest_id: ManifestId,
    target_branch: &str,
    path: &str,
    envs: &UnityEnvPair,
) -> Result<StructuredTree> {
    use transform::Transformer;

    tokio::try_join!(
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

    let UnityEnvPair {
        base: (base_env, base_data_dir),
        target: (target_env, target_data_dir),
    } = envs.clone();
    match transform::tools::transformer_for(path, None) {
        Some(Transformer::UnitySerialized) => {
            build_unity_serialized_diff(base_env, base_data_dir, target_env, target_data_dir, path)
                .await
        }
        Some(Transformer::UnityBundle) => {
            build_unity_bundle_diff(base_env, base_data_dir, target_env, target_data_dir, path)
                .await
        }
        _ => unreachable!("deep diff only calls this for is_deep_comparable paths"),
    }
}
