// TODO(ai-review): review for style and correctness
//! Query-cache entries for a frontend URL, built only from data the backend has warm.
//! Cold data is left out and fetched by the frontend as usual, so a preload never
//! delays the page.

use std::sync::Arc;
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
use crate::routes::files::file_view_of;
use crate::routes::game_info;
use crate::routes::library::{
    AppInfo, ManifestRef, app_info_cache_key, app_manifest_refs, indexed_statuses,
    library_cache_key, manifest_files_of, manifest_info_of, manifest_status_of,
};
use crate::state::{AppState, Snapshot};
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
    /// `data` covers only part of what the route returns; the frontend shows it and refetches.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
}

impl Entry {
    fn current(key: Value, data: impl Serialize) -> Result<Self> {
        Ok(Self {
            key,
            data: serde_json::to_value(data)?,
            updated_at: None,
            incomplete: false,
        })
    }

    fn as_of(key: Value, data: Value, fetched: SystemTime) -> Result<Self> {
        Ok(Self {
            key,
            data,
            updated_at: Some(fetched.duration_since(UNIX_EPOCH)?.as_millis().try_into()?),
            incomplete: false,
        })
    }
}

/// Query keys, mirrored by `frontend/src/data/queryKeys.ts`.
mod keys {
    use serde_json::{Value, json};

    use crate::routes::library::ManifestRef;
    use crate::steam::{AppId, DepotId, ManifestId};

    pub fn library() -> Value {
        json!(["library"])
    }

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

    pub fn file_view(
        app: AppId,
        depot: DepotId,
        manifest: ManifestId,
        branch: &str,
        path: &str,
    ) -> Value {
        json!(["file-view", app, depot, manifest, branch, path])
    }

    pub fn game_info(app: AppId, depot: DepotId, manifest: ManifestId) -> Value {
        json!(["game-info", app, depot, manifest])
    }

    pub fn app_manifest_statuses(app: AppId) -> Value {
        json!(["manifest-statuses", app])
    }

    pub fn manifest_statuses(app: AppId, refs: &[ManifestRef]) -> Value {
        json!(["manifest-statuses", app, refs])
    }
}

#[derive(Deserialize)]
struct ManifestPageQuery {
    branch: Option<String>,
    path: Option<String>,
    target_depot_id: Option<u32>,
    target_manifest_id: Option<u64>,
    target_branch: Option<String>,
}

/// Files up to this size get their view (including inline text) preloaded into the HTML.
const PRELOAD_FILE_CAP_BYTES: u64 = 1024 * 1024;

pub async fn entries(state: &AppState, uri: &Uri) -> Result<Vec<Entry>> {
    let mut entries = global(state)?;
    entries.extend(route(state, uri).await?);
    Ok(entries)
}

/// In-memory state every page reads, e.g. the auth status the root route waits on.
fn global(state: &AppState) -> Result<Vec<Entry>> {
    let auth = AuthStatus::from_state(state);
    let library = match &auth.account {
        Some(account) => state
            .disk_cache
            .read::<Value>(&library_cache_key(account))?,
        None => None,
    };
    let mut entries = vec![
        Entry::current(keys::auth(), &auth)?,
        Entry::current(keys::mount_status(), state.mount.status())?,
        Entry::current(keys::export_status(), state.exports.current())?,
        Entry::current(
            keys::config(),
            build_config_dto(state, Config::load_or_default()?),
        )?,
    ];
    // The frontend prefetches the library on every page.
    if let Some((games, fetched)) = library {
        entries.push(Entry::as_of(keys::library(), games, fetched)?);
    }
    Ok(entries)
}

async fn route(state: &AppState, uri: &Uri) -> Result<Vec<Entry>> {
    let segments: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    let ["apps", app, rest @ ..] = segments.as_slice() else {
        return Ok(Vec::new());
    };
    let Ok(app) = app.parse().map(AppId) else {
        return Ok(Vec::new());
    };
    let mut entries = app_pages(state, app, rest.is_empty())?;
    if let ["depots", depot, "history"] = rest
        && let Ok(depot) = depot.parse()
    {
        entries.extend(depot_game_infos(state, app, DepotId(depot))?);
    }
    if let ["depots", depot, "manifests", manifest, subpage @ ..] = rest
        && let (Ok(depot), Ok(manifest)) = (depot.parse(), manifest.parse())
    {
        let query = Query::<ManifestPageQuery>::try_from_uri(uri)?.0;
        entries.extend(
            manifest_pages(
                state,
                app,
                DepotId(depot),
                ManifestId(manifest),
                subpage,
                query,
            )
            .await?,
        );
    }
    Ok(entries)
}

/// App info as last fetched from Steam (refetched by the frontend once outdated) and the
/// user-tracked extras. The status batch only on the overview, where it fills the table;
/// elsewhere it feeds closed dropdowns and would delay the HTML by its computation.
fn app_pages(state: &AppState, app: AppId, overview: bool) -> Result<Vec<Entry>> {
    let extras = extra_manifests_of(state, app);
    let mut entries = vec![Entry::current(keys::extra_manifests(app), &extras)?];
    if let Some((info, fetched)) = state.disk_cache.read::<AppInfo>(&app_info_cache_key(app))? {
        if overview
            && let Some((statuses, complete)) =
                indexed_statuses(state, app, &app_manifest_refs(&info, &extras))
        {
            entries.push(Entry {
                incomplete: !complete,
                ..Entry::current(keys::app_manifest_statuses(app), statuses)?
            });
        }
        entries.push(Entry::as_of(
            keys::app(app),
            serde_json::to_value(info)?,
            fetched,
        )?);
    }
    Ok(entries)
}

/// Game info of every manifest of `depot` in the disk cache, for pages listing its versions.
fn depot_game_infos(state: &AppState, app: AppId, depot: DepotId) -> Result<Vec<Entry>> {
    state
        .disk_cache
        .list::<Value>(&game_info::cache_dir(depot))?
        .into_iter()
        .map(|(manifest, info)| {
            Entry::current(
                keys::game_info(app, depot, ManifestId(manifest.parse()?)),
                info,
            )
        })
        .collect()
}

/// The manifest page and its file, diff and file-history subpages.
async fn manifest_pages(
    state: &AppState,
    app: AppId,
    depot: DepotId,
    manifest: ManifestId,
    subpage: &[&str],
    query: ManifestPageQuery,
) -> Result<Vec<Entry>> {
    let branch = query.branch.unwrap_or_else(|| "public".to_owned());
    let mut entries = cached_game_info(state, app, depot, manifest)?;
    if subpage == ["file", "history"] {
        entries.extend(depot_game_infos(state, app, depot)?);
    }
    if subpage == ["diff"]
        && let (Some(target_depot), Some(target_manifest)) =
            (query.target_depot_id, query.target_manifest_id)
    {
        let (target_depot, target_manifest) = (DepotId(target_depot), ManifestId(target_manifest));
        let target_branch = query.target_branch.unwrap_or_else(|| "public".to_owned());
        entries.extend(cached_game_info(state, app, target_depot, target_manifest)?);
        if let Some(target) =
            open_if_on_disk(state, app, target_depot, target_manifest, &target_branch).await?
        {
            entries.push(status_entry(state, app, &target, &target_branch)?);
            if let Some(path) = &query.path {
                entries
                    .extend(file_view_if_local(state, app, &target, &target_branch, path).await?);
            }
        }
    }
    let Some(snapshot) = open_if_on_disk(state, app, depot, manifest, &branch).await? else {
        return Ok(entries);
    };
    entries.push(status_entry(state, app, &snapshot, &branch)?);
    match subpage {
        [] => entries.extend([
            Entry::current(
                keys::manifest_info(app, depot, manifest, &branch),
                manifest_info_of(&snapshot),
            )?,
            Entry::current(
                keys::manifest_files(app, depot, manifest, &branch),
                manifest_files_of(&snapshot),
            )?,
        ]),
        ["file"] | ["diff"] => {
            if let Some(path) = &query.path {
                entries.extend(file_view_if_local(state, app, &snapshot, &branch, path).await?);
            }
        }
        _ => {}
    }
    Ok(entries)
}

fn cached_game_info(
    state: &AppState,
    app: AppId,
    depot: DepotId,
    manifest: ManifestId,
) -> Result<Vec<Entry>> {
    let Some((info, _)) = state
        .disk_cache
        .read::<Value>(&game_info::cache_key(depot, manifest))?
    else {
        return Ok(Vec::new());
    };
    Ok(vec![Entry::current(
        keys::game_info(app, depot, manifest),
        info,
    )?])
}

/// The manifest when it is on disk; opening needs the Steam client even then.
async fn open_if_on_disk(
    state: &AppState,
    app: AppId,
    depot: DepotId,
    manifest: ManifestId,
    branch: &str,
) -> Result<Option<Arc<Snapshot>>> {
    let on_disk = state
        .store_index
        .read()
        .expect("store_index poisoned")
        .has_manifest(app, depot, manifest);
    if !on_disk {
        return Ok(None);
    }
    Ok(Some(Arc::new(
        state.open_manifest(app, depot, manifest, branch).await?,
    )))
}

/// The single-manifest status entry `useManifestStatus` reads.
fn status_entry(state: &AppState, app: AppId, snapshot: &Snapshot, branch: &str) -> Result<Entry> {
    let status = manifest_status_of(state, snapshot);
    let manifest_ref = ManifestRef {
        depot_id: status.depot_id,
        manifest_id: status.manifest_id,
        branch: branch.to_owned(),
    };
    Entry::current(keys::manifest_statuses(app, &[manifest_ref]), [status])
}

async fn file_view_if_local(
    state: &AppState,
    app: AppId,
    snapshot: &Arc<Snapshot>,
    branch: &str,
    path: &str,
) -> Result<Option<Entry>> {
    if !file_is_local(state, snapshot, path) {
        return Ok(None);
    }
    let view = file_view_of(state, Arc::clone(snapshot), path, true)
        .await
        .map_err(|e| anyhow::anyhow!("file view of {path}: {}", e.message))?;
    let manifest = snapshot.manifest();
    Ok(Some(Entry::current(
        keys::file_view(
            app,
            DepotId(manifest.depot_id),
            ManifestId(manifest.manifest_id),
            branch,
            path,
        ),
        view,
    )?))
}

/// Whether `path` is a file below the preload cap with every chunk on disk, so building its
/// view downloads nothing.
fn file_is_local(state: &AppState, snapshot: &Snapshot, path: &str) -> bool {
    let Some(file) = snapshot.manifest().files.iter().find(|f| f.path == path) else {
        return false;
    };
    let index = state.store_index.read().expect("store_index poisoned");
    file.size <= PRELOAD_FILE_CAP_BYTES && file.chunks().iter().all(|c| index.has_chunk(&c.sha))
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
            "library": keys::library(),
            "auth": keys::auth(),
            "mountStatus": keys::mount_status(),
            "exportStatus": keys::export_status(),
            "config": keys::config(),
            "manifestInfo": keys::manifest_info(app, depot, manifest, branch),
            "manifestFiles": keys::manifest_files(app, depot, manifest, branch),
            "gameInfo": keys::game_info(app, depot, manifest),
            "fileView": keys::file_view(app, depot, manifest, branch, "Data/a.json"),
            "manifestStatuses": keys::manifest_statuses(app, &[self_ref]),
            "appManifestStatuses": keys::app_manifest_statuses(app),
        });
        assert_eq!(actual, contract);
    }
}
