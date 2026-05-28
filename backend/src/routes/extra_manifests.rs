// TODO(ai-review): review for style and correctness
//! User-tracked manifests not exposed by Steam's current branch list
//! — e.g. private/historical builds the account still has access to.

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::json;
use utoipa::ToSchema;

use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

use super::Result;

#[derive(Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "depot_id": 1234567,
    "manifest_id": "9876543210987654321",
    "branch": "public"
}))]
pub struct ExtraManifestEntryDto {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub branch: Option<String>,
}

impl From<crate::state::extra_manifests::ExtraManifestEntry> for ExtraManifestEntryDto {
    fn from(e: crate::state::extra_manifests::ExtraManifestEntry) -> Self {
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

impl From<ExtraManifestEntryDto> for crate::state::extra_manifests::ExtraManifestEntry {
    fn from(e: ExtraManifestEntryDto) -> Self {
        Self {
            depot_id: e.depot_id,
            manifest_id: e.manifest_id,
            branch: e.branch,
        }
    }
}

/// List tracked manifests
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/extra_manifests",
    tag = "extra-manifests",
    responses((status = 200, body = Vec<ExtraManifestEntryDto>))
)]
pub async fn get_extra_manifests(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
) -> Result<Json<Vec<ExtraManifestEntryDto>>> {
    let list = state.extra_manifests.list(appid);
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

/// Replace tracked manifest list
#[utoipa::path(
    put,
    path = "/api/apps/{appid}/extra_manifests",
    tag = "extra-manifests",
    request_body = ExtraManifestsRequest,
    responses((status = 200, body = Vec<ExtraManifestEntryDto>))
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

/// Remove one tracked manifest
#[utoipa::path(
    delete,
    path = "/api/apps/{appid}/extra_manifests/{depot_id}/{manifest_id}",
    tag = "extra-manifests",
    responses((status = 200, body = Vec<ExtraManifestEntryDto>))
)]
pub async fn delete_extra_manifest(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
) -> Result<Json<Vec<ExtraManifestEntryDto>>> {
    let saved = state.extra_manifests.remove(appid, depot_id, manifest_id)?;
    Ok(Json(saved.into_iter().map(Into::into).collect()))
}
