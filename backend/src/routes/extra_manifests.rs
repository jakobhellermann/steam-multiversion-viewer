// TODO(ai-review): review for style and correctness
//! User-tracked manifests not exposed by Steam's current branch list
//! — e.g. private/historical builds the account still has access to.

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

use super::Result;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ExtraManifestEntryDto {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub branch: Option<String>,
}

impl From<crate::extra_manifests::ExtraManifestEntry> for ExtraManifestEntryDto {
    fn from(e: crate::extra_manifests::ExtraManifestEntry) -> Self {
        Self {
            depot_id: e.depot_id,
            manifest_id: e.manifest_id,
            branch: e.branch,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct ExtraManifestsRequest {
    pub entries: Vec<ExtraManifestEntryDto>,
}

impl From<ExtraManifestEntryDto> for crate::extra_manifests::ExtraManifestEntry {
    fn from(e: ExtraManifestEntryDto) -> Self {
        Self {
            depot_id: e.depot_id,
            manifest_id: e.manifest_id,
            branch: e.branch,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/apps/{appid}/extra_manifests",
    tag = "extra-manifests"
)]
pub async fn get_extra_manifests(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
) -> Result<Json<Vec<ExtraManifestEntryDto>>> {
    let list = state.extra_manifests.list(appid);
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

/// Replace the full list of user-tracked manifests for this app.
#[utoipa::path(
    put,
    path = "/api/apps/{appid}/extra_manifests",
    tag = "extra-manifests",
    request_body = ExtraManifestsRequest
)]
pub async fn put_extra_manifests(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ExtraManifestsRequest>,
) -> Result<Json<Vec<ExtraManifestEntryDto>>> {
    let entries = body.entries.into_iter().map(Into::into).collect();
    let saved = state.extra_manifests.set(appid, entries)?;
    Ok(Json(saved.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    delete,
    path = "/api/apps/{appid}/extra_manifests/{depot_id}/{manifest_id}",
    tag = "extra-manifests"
)]
pub async fn delete_extra_manifest(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
) -> Result<Json<Vec<ExtraManifestEntryDto>>> {
    let saved = state.extra_manifests.remove(appid, depot_id, manifest_id)?;
    Ok(Json(saved.into_iter().map(Into::into).collect()))
}
