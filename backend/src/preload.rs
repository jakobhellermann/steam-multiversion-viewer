// TODO(ai-review): review for style and correctness
//! Query-cache entries for a frontend URL, built only from data the backend has warm.
//! Cold data is left out and fetched by the frontend as usual, so a preload never
//! delays the page.

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::Uri;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::routes::auth::AuthStatus;
use crate::routes::config::build_config_dto;
use crate::routes::game_info::{self, GameInfo};
use crate::routes::library::{
    ManifestRef, manifest_files_of, manifest_info_of, manifest_status_of,
};
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

/// One TanStack Query cache entry: `data` is exactly what the API route for `key` returns.
#[derive(Debug, Serialize)]
pub struct Entry {
    pub key: Value,
    pub data: Value,
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
        Entry {
            key: keys::auth(),
            data: serde_json::to_value(AuthStatus::from_state(state))?,
        },
        Entry {
            key: keys::mount_status(),
            data: serde_json::to_value(state.mount.status())?,
        },
        Entry {
            key: keys::export_status(),
            data: serde_json::to_value(state.exports.current())?,
        },
        Entry {
            key: keys::config(),
            data: serde_json::to_value(build_config_dto(state, Config::load_or_default()?))?,
        },
    ])
}

async fn route(state: &AppState, uri: &Uri) -> Result<Vec<Entry>> {
    let segments: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    match segments.as_slice() {
        ["apps", app, "depots", depot, "manifests", manifest, ..] => {
            let (Ok(app), Ok(depot), Ok(manifest)) = (app.parse(), depot.parse(), manifest.parse())
            else {
                return Ok(Vec::new());
            };
            let branch = Query::<BranchQuery>::try_from_uri(uri)?
                .0
                .branch
                .unwrap_or_else(|| "public".to_owned());
            manifest_page(
                state,
                AppId(app),
                DepotId(depot),
                ManifestId(manifest),
                branch,
            )
            .await
        }
        _ => Ok(Vec::new()),
    }
}

async fn manifest_page(
    state: &AppState,
    app: AppId,
    depot: DepotId,
    manifest: ManifestId,
    branch: String,
) -> anyhow::Result<Vec<Entry>> {
    let on_disk = state
        .store_index
        .read()
        .expect("store_index poisoned")
        .has_manifest(app, depot, manifest);
    if !on_disk || state.steam.load().is_none() {
        return Ok(Vec::new());
    }
    let snapshot = state.open_manifest(app, depot, manifest, &branch).await?;
    let self_ref = ManifestRef {
        depot_id: depot,
        manifest_id: manifest,
        branch: branch.clone(),
    };
    let mut entries = vec![
        Entry {
            key: keys::manifest_info(app, depot, manifest, &branch),
            data: serde_json::to_value(manifest_info_of(&snapshot))?,
        },
        Entry {
            key: keys::manifest_files(app, depot, manifest, &branch),
            data: serde_json::to_value(manifest_files_of(&snapshot))?,
        },
        Entry {
            key: keys::manifest_statuses(app, &[self_ref]),
            data: serde_json::to_value([manifest_status_of(state, &snapshot)])?,
        },
    ];
    if let Some((info, _)) = state
        .disk_cache
        .read::<GameInfo>(&game_info::cache_key(depot, manifest))?
    {
        entries.push(Entry {
            key: keys::game_info(app, depot, manifest),
            data: serde_json::to_value(info)?,
        });
    }
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
