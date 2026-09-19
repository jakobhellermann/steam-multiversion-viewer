// TODO(ai-review): review for style and correctness
//! Unity scratch resolution + blocking-task helper for the structured
//! routes. The rabex `Environment` is synchronous, so its users hop
//! into `spawn_blocking` with an already-resolved per-manifest scratch.

use std::sync::Arc;

use crate::http::ApiError;
use crate::routes::Result;
use crate::state::manifest_cache::{ManifestScratch, UnityScratch};
use crate::state::{AppState, Snapshot};
use crate::steam::{AppId, DepotId, ManifestId};

/// Resolve the per-manifest unity scratch + data dir; 415s when the
/// manifest isn't a unity game.
pub fn scratch_side(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    snapshot: Arc<Snapshot>,
) -> Result<(Arc<ManifestScratch>, String)> {
    let scratch = state
        .manifest_cache
        .scratch(appid, depot_id, manifest_id, branch);
    let unity = scratch
        .unity(snapshot)
        .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
    let data_dir = unity.data_dir();
    Ok((scratch, data_dir))
}

/// Run `f` on the blocking pool with the manifest's unity scratch.
/// `task` names the closure in panic error messages.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_unity_blocking<T, F>(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    snapshot: Arc<Snapshot>,
    task: &'static str,
    f: F,
) -> Result<T>
where
    F: FnOnce(&UnityScratch, &str) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let (scratch, data_dir) = scratch_side(state, appid, depot_id, manifest_id, branch, snapshot)?;
    let f = tokio::task::spawn_blocking(move || {
        let unity = scratch
            .unity_already_initialized()
            .expect("unity scratch was initialised on the async side");
        f(unity, &data_dir)
    });
    f.await
        .map_err(|e| ApiError::internal(format!("{task} task panicked: {e}")))?
        .map_err(|e| ApiError::internal(e.to_string()))
}
