// TODO(ai-review): review for style and correctness
//! Query-cache entries for a frontend URL, built only from data the backend has warm.
//! Cold data is left out and fetched by the frontend as usual, so a preload never
//! delays the page.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::Uri;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::routes::auth::AuthStatus;
use crate::routes::config::build_config_dto;
use crate::routes::extra_manifests::extra_manifests_of;
use crate::routes::game_info;
use crate::routes::library::{
    ManifestRef, app_info_cache_key, manifest_files_of, manifest_info_of, manifest_status_of,
};
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

/// One TanStack Query cache entry: `data` is exactly what the API route for `key` returns.
#[derive(Debug, Serialize)]
pub struct Entry {
    pub key: Value,
    pub data: Value,
    /// When `data` was fetched (ms since the epoch), for data that can be outdated; absent
    /// means current.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

impl Entry {
    fn current(key: Value, data: impl Serialize) -> Result<Self> {
        Ok(Self {
            key,
            data: serde_json::to_value(data)?,
            updated_at: None,
        })
    }

    fn as_of(key: Value, data: Value, fetched: SystemTime) -> Result<Self> {
        Ok(Self {
            key,
            data,
            updated_at: Some(fetched.duration_since(UNIX_EPOCH)?.as_millis().try_into()?),
        })
    }
}

/// Query keys, mirrored by `frontend/src/data/queryKeys.ts`.
mod keys {
    use serde_json::{Value, json};

    use crate::routes::library::ManifestRef;
    use crate::steam::{AppId, DepotId, ManifestId};

    pub fn auth() -> Value {
        json!(["auth"])
    }

    pub fn mount_status() -> Value {
        json!(["mount-status"])
    }

    pub fn export_status() -> Value {
        json!(["export-status"])
    }

    pub fn config() -> Value {
        json!(["config"])
    }

    pub fn app(app: AppId) -> Value {
        json!(["app", app])
    }

    pub fn extra_manifests(app: AppId) -> Value {
        json!(["extra-manifests", app])
    }

    pub fn manifest_info(app: AppId, depot: DepotId, manifest: ManifestId, branch: &str) -> Value {
        json!(["manifest-info", app, depot, manifest, branch])
    }

    pub fn manifest_files(app: AppId, depot: DepotId, manifest: ManifestId, branch: &str) -> Value {
        json!(["manifest-files", app, depot, manifest, branch])
    }

    pub fn game_info(app: AppId, depot: DepotId, manifest: ManifestId) -> Value {
        json!(["game-info", app, depot, manifest])
    }

    pub fn manifest_statuses(app: AppId, refs: &[ManifestRef]) -> Value {
        json!(["manifest-statuses", app, refs])
    }
}

#[derive(Deserialize)]
struct BranchQuery {
    branch: Option<String>,
}

pub async fn entries(state: &AppState, uri: &Uri) -> Result<Vec<Entry>> {
    let mut entries = global(state)?;
    entries.extend(route(state, uri).await?);
    Ok(entries)
}

/// In-memory state every page reads, e.g. the auth status the root route waits on.
fn global(state: &AppState) -> Result<Vec<Entry>> {
    Ok(vec![
        Entry::current(keys::auth(), AuthStatus::from_state(state))?,
        Entry::current(keys::mount_status(), state.mount.status())?,
        Entry::current(keys::export_status(), state.exports.current())?,
        Entry::current(
            keys::config(),
            build_config_dto(state, Config::load_or_default()?),
        )?,
    ])
}

async fn route(state: &AppState, uri: &Uri) -> Result<Vec<Entry>> {
    let segments: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    let ["apps", app, rest @ ..] = segments.as_slice() else {
        return Ok(Vec::new());
    };
    let Ok(app) = app.parse().map(AppId) else {
        return Ok(Vec::new());
    };
    let mut entries = app_pages(state, app)?;
    if let ["depots", depot, "manifests", manifest, ..] = rest
        && let (Ok(depot), Ok(manifest)) = (depot.parse(), manifest.parse())
    {
        let branch = Query::<BranchQuery>::try_from_uri(uri)?
            .0
            .branch
            .unwrap_or_else(|| "public".to_owned());
        entries
            .extend(manifest_page(state, app, DepotId(depot), ManifestId(manifest), branch).await?);
    }
    Ok(entries)
}

/// App info as last fetched from Steam (refetched by the frontend once outdated) and the
/// user-tracked extras.
fn app_pages(state: &AppState, app: AppId) -> Result<Vec<Entry>> {
    let mut entries = vec![Entry::current(
        keys::extra_manifests(app),
        extra_manifests_of(state, app),
    )?];
    if let Some((info, fetched)) = state.disk_cache.read::<Value>(&app_info_cache_key(app))? {
        entries.push(Entry::as_of(keys::app(app), info, fetched)?);
    }
    Ok(entries)
}

async fn manifest_page(
    state: &AppState,
    app: AppId,
    depot: DepotId,
    manifest: ManifestId,
    branch: String,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    if let Some((info, _)) = state
        .disk_cache
        .read::<Value>(&game_info::cache_key(depot, manifest))?
    {
        entries.push(Entry::current(keys::game_info(app, depot, manifest), info)?);
    }
    // Opening a manifest needs the Steam client even when it is on disk.
    let on_disk = state
        .store_index
        .read()
        .expect("store_index poisoned")
        .has_manifest(app, depot, manifest);
    if !on_disk || state.steam.load().is_none() {
        return Ok(entries);
    }
    let snapshot = state.open_manifest(app, depot, manifest, &branch).await?;
    let self_ref = ManifestRef {
        depot_id: depot,
        manifest_id: manifest,
        branch: branch.clone(),
    };
    entries.extend([
        Entry::current(
            keys::manifest_info(app, depot, manifest, &branch),
            manifest_info_of(&snapshot),
        )?,
        Entry::current(
            keys::manifest_files(app, depot, manifest, &branch),
            manifest_files_of(&snapshot),
        )?,
        Entry::current(
            keys::manifest_statuses(app, &[self_ref]),
            [manifest_status_of(state, &snapshot)],
        )?,
    ]);
    Ok(entries)
}

#[derive(Deserialize)]
pub struct PreloadQuery {
    /// Frontend URL (path and query) to preload for.
    url: String,
}

/// Preload entries for `?url=`, for dev servers that render `index.html` themselves.
pub async fn preload(
    State(state): State<AppState>,
    Query(q): Query<PreloadQuery>,
) -> Result<Json<Vec<Entry>>, crate::http::ApiError> {
    let uri: Uri = q
        .url
        .parse()
        .map_err(|e| crate::http::ApiError::bad_request(format!("invalid url: {e}")))?;
    let entries = entries(&state, &uri)
        .await
        .map_err(|e| crate::http::ApiError::internal(e.to_string()))?;
    Ok(Json(entries))
}

/// `entries` as a `<script>` for `index.html`; `<` is escaped so the JSON can't end the tag.
pub fn script_tag(entries: &[Entry]) -> String {
    let json = serde_json::to_string(entries)
        .expect("JSON values always serialize")
        .replace('<', "\\u003c");
    format!(r#"<script id="preload" type="application/json">{json}</script>"#)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::keys;
    use crate::routes::library::ManifestRef;
    use crate::steam::{AppId, DepotId, ManifestId};

    #[test]
    fn keys_match_frontend_contract() {
        let contract: Value = serde_json::from_str(include_str!(
            "../../frontend/src/data/queryKeys.contract.json"
        ))
        .unwrap();
        let (app, depot, manifest, branch) = (
            AppId(1030300),
            DepotId(1030303),
            ManifestId(5954103139200615141),
            "beta",
        );
        let self_ref = ManifestRef {
            depot_id: depot,
            manifest_id: manifest,
            branch: branch.to_owned(),
        };
        let actual = serde_json::json!({
            "app": keys::app(app),
            "extraManifests": keys::extra_manifests(app),
            "auth": keys::auth(),
            "mountStatus": keys::mount_status(),
            "exportStatus": keys::export_status(),
            "config": keys::config(),
            "manifestInfo": keys::manifest_info(app, depot, manifest, branch),
            "manifestFiles": keys::manifest_files(app, depot, manifest, branch),
            "gameInfo": keys::game_info(app, depot, manifest),
            "manifestStatuses": keys::manifest_statuses(app, &[self_ref]),
        });
        assert_eq!(actual, contract);
    }
}
