// TODO(ai-review): review for style and correctness
//! `POST` diff endpoints — manifest-level path diffs, per-file
//! cross-manifest status, and the unified text diff for a single file.
//!
//! All three sit under `/api/apps/{appid}/...` and read manifests via
//! [`crate::state::AppState::open_manifest`] so cache hits short-circuit
//! the network roundtrip.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
use steam_vent_depot::{DepotFile, FileKind};
use tokio::sync::Semaphore;
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::state::AppState;
use crate::steam::{AppId, DepotId, ManifestId};

use super::{FileViewQuery, ManifestRef, Result};

#[derive(Deserialize, ToSchema)]
pub struct ManifestDiffRequest {
    /// The manifest whose paths we report. A path is included if the file
    /// under that path differs from *any* of the listed others (added,
    /// removed, or content-changed).
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestDiffResponse {
    /// Paths in `base` that differ from at least one `other`. Includes
    /// paths missing in `base` but present in some `other` ("removed").
    pub changed_paths: Vec<String>,
}

/// Symmetric path-level diff between a base manifest and a set of others.
/// Returns every path that, in at least one of the other manifests, is
/// either missing or has a different content fingerprint. Identity is
/// `(kind, size, sha, linktarget)`; absence on either side counts as a
/// difference.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/diff",
    request_body = ManifestDiffRequest
)]
#[tracing::instrument(skip_all, fields(others = body.others.len()))]
pub async fn manifest_diff(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ManifestDiffRequest>,
) -> Result<Json<ManifestDiffResponse>> {
    let base_snap = state
        .open_manifest(
            appid,
            body.base.depot_id,
            body.base.manifest_id,
            &body.base.branch,
        )
        .await?;

    // Open all `other` manifests in parallel — pulling them one by one
    // on a cold cache adds up fast when comparing across many depots.
    let sem = Arc::new(Semaphore::new(8));
    let mut fu = FuturesUnordered::new();
    let mut seen = HashSet::new();
    for r in &body.others {
        if !seen.insert((r.depot_id, r.manifest_id)) {
            continue;
        }
        if (r.depot_id, r.manifest_id) == (body.base.depot_id, body.base.manifest_id) {
            // Comparing a manifest against itself yields nothing.
            continue;
        }
        let state = &state;
        let sem = sem.clone();
        let depot_id = r.depot_id;
        let manifest_id = r.manifest_id;
        let branch = r.branch.clone();
        fu.push(async move {
            let _permit = sem.acquire().await.expect("semaphore not closed");
            state
                .open_manifest(appid, depot_id, manifest_id, &branch)
                .await
        });
    }

    let base = base_snap.manifest();
    let mut base_by_path: HashMap<&str, &DepotFile> = HashMap::with_capacity(base.files.len());
    for f in &base.files {
        if matches!(f.kind, FileKind::Directory) {
            continue;
        }
        base_by_path.insert(f.path.as_str(), f);
    }

    // Identity tuple used to decide "same content". Files without a sha
    // (e.g. symlinks) still get a stable fingerprint via the rest.
    fn fp(f: &DepotFile) -> (FileKind, u64, Option<[u8; 20]>, Option<&str>) {
        (f.kind, f.size, f.sha, f.linktarget.as_deref())
    }

    let mut changed: HashSet<String> = HashSet::new();
    while let Some(result) = fu.next().await {
        let snap = result?;
        let other = snap.manifest();
        let mut other_paths: HashSet<&str> = HashSet::with_capacity(other.files.len());
        for f in &other.files {
            if matches!(f.kind, FileKind::Directory) {
                continue;
            }
            other_paths.insert(f.path.as_str());
            match base_by_path.get(f.path.as_str()) {
                None => {
                    // Present in `other`, absent in `base` — counts as a
                    // change of `base`'s view (the file would "appear").
                    changed.insert(f.path.clone());
                }
                Some(base_f) => {
                    if fp(base_f) != fp(f) {
                        changed.insert(f.path.clone());
                    }
                }
            }
        }
        // Paths in `base` that this `other` does not have at all.
        for path in base_by_path.keys() {
            if !other_paths.contains(path) {
                changed.insert((*path).to_owned());
            }
        }
    }

    let mut changed_paths: Vec<String> = changed.into_iter().collect();
    changed_paths.sort();
    Ok(Json(ManifestDiffResponse { changed_paths }))
}

#[derive(Deserialize, ToSchema)]
pub struct FileDiffTargetsRequest {
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
    pub path: String,
}

#[derive(Serialize, ToSchema)]
pub struct FileDiffTargetsResponse {
    /// For each `other` manifest, whether the file at `path` differs
    /// from the base (`different`), doesn't exist there (`missing`), or
    /// is identical (`same`). Identity is the file's content sha (with
    /// kind/linktarget tie-breakers for non-file or symlink entries).
    pub statuses: Vec<FileDiffTargetStatus>,
}

#[derive(Serialize, ToSchema)]
pub struct FileDiffTargetStatus {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub status: FileDiffStatus,
}

#[derive(Serialize, ToSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum FileDiffStatus {
    Same,
    Different,
    Missing,
}

/// Per-target diff status for a single file across many manifests.
/// One round trip instead of fanning out N file-view requests — useful
/// for callers that want to filter a long candidate list down to the
/// manifests where a specific file actually changed.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/file/diff-targets",
    request_body = FileDiffTargetsRequest
)]
#[tracing::instrument(skip_all, fields(path = %body.path, others = body.others.len()))]
pub async fn file_diff_targets(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<FileDiffTargetsRequest>,
) -> Result<Json<FileDiffTargetsResponse>> {
    let base_snap = state
        .open_manifest(
            appid,
            body.base.depot_id,
            body.base.manifest_id,
            &body.base.branch,
        )
        .await?;
    let base_file = base_snap
        .manifest()
        .files
        .iter()
        .find(|f| f.path == body.path)
        .cloned();

    // Identity tuple — same as manifest_diff, scoped to one file.
    let base_fp = base_file
        .as_ref()
        .map(|f| (f.kind, f.size, f.sha, f.linktarget.clone()));

    let sem = Arc::new(Semaphore::new(8));
    let mut fu = FuturesUnordered::new();
    let mut seen = HashSet::new();
    for r in &body.others {
        if !seen.insert((r.depot_id, r.manifest_id)) {
            continue;
        }
        let state = &state;
        let sem = sem.clone();
        let depot_id = r.depot_id;
        let manifest_id = r.manifest_id;
        let branch = r.branch.clone();
        fu.push(async move {
            let _permit = sem.acquire().await.expect("semaphore not closed");
            let result = state
                .open_manifest(appid, depot_id, manifest_id, &branch)
                .await;
            (depot_id, manifest_id, result)
        });
    }

    let path = body.path.as_str();
    let mut statuses = Vec::new();
    while let Some((depot_id, manifest_id, result)) = fu.next().await {
        let status = match result {
            Ok(snap) => {
                let file = snap
                    .manifest()
                    .files
                    .iter()
                    .find(|f| f.path == path)
                    .cloned();
                let other_fp = file.map(|f| (f.kind, f.size, f.sha, f.linktarget));
                match (&base_fp, &other_fp) {
                    (Some(b), Some(o)) if b == o => FileDiffStatus::Same,
                    (None, None) => FileDiffStatus::Same,
                    (_, None) => FileDiffStatus::Missing,
                    (None, Some(_)) => FileDiffStatus::Different,
                    (Some(_), Some(_)) => FileDiffStatus::Different,
                }
            }
            Err(err) => {
                tracing::warn!(%depot_id, %manifest_id, %err, "open_manifest failed in file diff");
                // Treat a fetch error as "different" so the user still
                // sees the candidate — they can investigate.
                FileDiffStatus::Different
            }
        };
        statuses.push(FileDiffTargetStatus {
            depot_id,
            manifest_id,
            status,
        });
    }

    Ok(Json(FileDiffTargetsResponse { statuses }))
}

#[derive(Deserialize, ToSchema)]
pub struct FileDiffRequest {
    pub path: String,
    pub target: ManifestRef,
}

/// Unified diff between the file at `path` in the base manifest and in
/// `body.target`. Both sides go through the same text-resolution as
/// `/file/transformed` — if a transformer is registered, the diff is on
/// its output. Response body is a unified-diff text blob with
/// `Content-Type: text/x-diff`.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/diff",
    params(FileViewQuery),
    request_body = FileDiffRequest,
)]
#[tracing::instrument(skip_all, fields(path = %body.path))]
pub async fn manifest_file_diff(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<FileViewQuery>,
    Json(body): Json<FileDiffRequest>,
) -> Result<Response> {
    // Both sides resolve through the same helper. We deliberately run
    // the two snapshot opens sequentially — `open_manifest` is cheap
    // when cached, and serialising keeps the depot-key fetch from
    // racing with itself.
    let base =
        resolve_diff_text(&state, appid, depot_id, manifest_id, &q.branch, &body.path).await?;
    let target = resolve_diff_text(
        &state,
        appid,
        body.target.depot_id,
        body.target.manifest_id,
        &body.target.branch,
        &body.path,
    )
    .await?;

    let base_label = diff_label(depot_id, manifest_id, base.creation_time);
    let target_label = diff_label(
        body.target.depot_id,
        body.target.manifest_id,
        target.creation_time,
    );
    // Always diff "older → newer" so `+` consistently means "added in
    // the newer version" regardless of which manifest the caller chose
    // to open. Equal timestamps fall back to the request order.
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
    /// Steam-side manifest creation timestamp (unix seconds). Used to
    /// pick the "older" side so the unified diff reads in the natural
    /// chronological direction.
    creation_time: u32,
}

/// Format the `--- foo` / `+++ bar` header line for one side of the
/// diff: `depot/<right-padded manifest id> YYYY-MM-DD`. The 20-char
/// pad covers the u64 max length so 19- and 20-digit ids line up.
fn diff_label(depot_id: DepotId, manifest_id: ManifestId, creation_time: u32) -> String {
    let date_fmt = time::macros::format_description!("[year]-[month]-[day]");
    let date = time::OffsetDateTime::from_unix_timestamp(creation_time as i64)
        .ok()
        .and_then(|d| d.format(&date_fmt).ok())
        .unwrap_or_else(|| "?".to_string());
    format!("{depot_id}/{:<20} {date}", manifest_id.0)
}

/// Resolve the file at `(depot_id, manifest_id, path)` to the text we
/// want to diff. Uses the transformer when one is registered (so a .dll
/// diff is between two decompiled C# blobs); falls back to the raw
/// UTF-8 bytes for plain text files; errors with 415 for anything
/// binary that has no transformer.
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
    let (file_path, file_sha, chunks_for_dl) = {
        let manifest = snapshot.manifest();
        let file = manifest
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or_else(|| ApiError {
                status: axum::http::StatusCode::NOT_FOUND,
                message: format!("file not in manifest {depot_id}/{manifest_id}: {path}"),
            })?;
        let sha = file.sha.ok_or_else(|| ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: format!("file has no content sha: {path}"),
        })?;
        (
            file.path.clone(),
            sha,
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
        )
    };

    if let Some(transformer) = crate::transform::tools::transformer_for(&file_path) {
        let cfg = state.config.load();
        if let Some(cached) = crate::transform::read_cached(&cfg.store_root, &file_sha)? {
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
            crate::transform::Transformer::Cli(tool) => {
                let bytes = snapshot.read_full(&file_path).await?;
                crate::transform::run_and_cache(&cfg.store_root, tool, &file_sha, &bytes)
                    .await
                    .map_err(|e| ApiError {
                        status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        message: e.to_string(),
                    })?
            }
            #[cfg(feature = "unity")]
            crate::transform::Transformer::UnitySerialized => {
                let snapshot = snapshot.clone();
                let file_path = file_path.clone();
                tokio::task::spawn_blocking(move || {
                    crate::unity::dump_unity_serialized(snapshot, &file_path)
                })
                .await
                .map_err(|e| ApiError {
                    status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    message: format!("unity dump task panicked: {e}"),
                })?
                .map_err(|e| ApiError {
                    status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    message: e.to_string(),
                })?
            }
            crate::transform::Transformer::Dll => {
                // .NET assemblies don't have a single text dump to
                // diff — they're inherently per-type. A future
                // structured-diff endpoint can cover this; for now
                // the file falls through to the byte-equality view.
                return Err(ApiError {
                    status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    message: ".NET assemblies have no text-diff representation yet".to_string(),
                });
            }
            #[cfg(feature = "unity")]
            crate::transform::Transformer::UnityBundle => {
                // Bundles contain multiple SerializedFiles; a single
                // text dump for diff is awkward and the structured
                // tree carries the actual signal. Punt for now.
                return Err(ApiError {
                    status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    message: "Unity bundles have no text-diff representation yet".to_string(),
                });
            }
        };
        return Ok(DiffSide {
            text,
            creation_time,
        });
    }

    // No transformer — only useful for files we can read as UTF-8.
    state
        .downloads
        .enqueue_and_wait(snapshot.clone(), chunks_for_dl)
        .await;
    let bytes = snapshot.read_full(&file_path).await?;
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| ApiError {
        status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
        message: format!("file is binary and has no registered transformer: {file_path}"),
    })?;
    Ok(DiffSide {
        text,
        creation_time,
    })
}

/// Query string for the structured-diff endpoint. Extends
/// [`FileViewQuery`] (which carries `path` + `branch` of the base) with
/// the target side, named as flat params so the whole call is GETtable
/// — POST would block the immutable Cache-Control from doing its job.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct StructuredDiffQuery {
    pub path: String,
    /// Branch of the base manifest. Defaults to `public` like
    /// [`FileViewQuery`].
    #[serde(default = "default_branch")]
    pub branch: String,
    pub target_depot_id: DepotId,
    pub target_manifest_id: ManifestId,
    #[serde(default = "default_branch")]
    pub target_branch: String,
}

fn default_branch() -> String {
    "public".to_string()
}

/// Structured per-object diff between two SerializedFiles. Returns a
/// pruned tree where each node is tagged `added`/`removed`/`changed`/
/// `unchanged`; unchanged leaves are dropped so the payload carries
/// only the spine to every difference.
///
/// Only `UnitySerialized` files are supported today; everything else
/// returns 415. Per-node content (the actual JSON dump for changed
/// objects) is fetched lazily by the client via the existing
/// `/file/structured/node` endpoint, once per side.
#[cfg(feature = "unity")]
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured-diff",
    params(StructuredDiffQuery)
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_structured_diff(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<StructuredDiffQuery>,
) -> Result<(
    crate::http::ImmutableCache,
    Json<crate::structured::StructuredTree>,
)> {
    use crate::transform::Transformer;

    let kind = crate::transform::tools::transformer_for(&q.path);

    // Open both manifests + pre-download the file's chunks on each
    // side in parallel — both passes are needed before we can hand the
    // pair to the blocking diff builder.
    let (base, target) = tokio::try_join!(
        prepare_structured_side(&state, appid, depot_id, manifest_id, &q.path, &q.branch),
        prepare_structured_side(
            &state,
            appid,
            q.target_depot_id,
            q.target_manifest_id,
            &q.path,
            &q.target_branch,
        ),
    )?;

    let diff = match kind {
        Some(Transformer::UnitySerialized) => {
            let path = q.path.clone();
            tokio::task::spawn_blocking(move || crate::unity::diff::build_diff(base, target, &path))
                .await
                .map_err(|e| ApiError {
                    status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    message: format!("structured-diff task panicked: {e}"),
                })?
                .map_err(|e| ApiError {
                    status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    message: e.to_string(),
                })?
        }
        Some(Transformer::Dll) => {
            let cfg = state.config.load();
            let store_root = cfg.store_root.clone();
            // dll-diff is GNU-conventional: `(in to only) → Added`.
            // The viewer's `base` is the URL-path manifest (what the
            // user is currently looking at) and `target` is the
            // `compare_to=` manifest. Aligning frontend semantics
            // (Added = in current view) with dll-diff means
            // `from = target` and `to = base`.
            let (to_bytes, to_sha) = dll_side_bytes(&base, &q.path).await?;
            let (from_bytes, from_sha) = dll_side_bytes(&target, &q.path).await?;
            let tree = crate::dll::diff::build_tree(
                &store_root,
                &from_sha,
                &from_bytes,
                &to_sha,
                &to_bytes,
                &q.path,
            )
            .await
            .map_err(|e| ApiError {
                status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                message: e.to_string(),
            })?;
            // Kick `ilspycmd -p` for both DLLs in the background.
            // Per-type lazy decompile calls from the node endpoint then
            // hit the cache instead of spawning a fresh `ilspycmd -t`.
            // Idempotent: skipped if already warming this sha.
            crate::dll::warm_full_decompile(&store_root, from_sha, from_bytes);
            crate::dll::warm_full_decompile(&store_root, to_sha, to_bytes);
            tree
        }
        _ => {
            return Err(ApiError {
                status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                message: format!("structured diff not supported for: {}", q.path),
            });
        }
    };

    Ok((crate::http::ImmutableCache, Json(diff)))
}

/// Read a file's content + manifest-recorded sha1 from one side of
/// the structured-diff pipeline. The sha keys the `decompile_type`
/// cache, so feeding the wrong one would silently duplicate every
/// per-type artefact.
async fn dll_side_bytes(
    snap: &Arc<crate::state::Snapshot>,
    path: &str,
) -> Result<(Vec<u8>, [u8; 20])> {
    let sha = snap
        .manifest()
        .files
        .iter()
        .find(|f| f.path == path)
        .and_then(|f| f.sha)
        .ok_or_else(|| ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: format!("file has no content sha: {path}"),
        })?;
    let bytes = snap.read_full(path).await?.to_vec();
    Ok((bytes, sha))
}

/// Per-node body for the Dll structured-diff route. `base_id` /
/// `target_id` are `type:<FQN>` ids from
/// [`crate::dll::diff::build_tree`]; we decompile the type via
/// `ilspycmd -t` on each side (cached) and return either a unified
/// diff of the two C# texts (both-sided) or the available text
/// verbatim (added/removed).
async fn dll_node_body(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    q: &StructuredDiffNodeQuery,
) -> Result<(crate::http::ImmutableCache, Response)> {
    fn parse_type_id(id: &str) -> Option<&str> {
        id.strip_prefix("type:")
    }
    let (base_inner, target_inner) = split_diff_id(&q.node_id);
    let base_type = base_inner.and_then(parse_type_id);
    let target_type = target_inner.and_then(parse_type_id);
    if base_type.is_none() && target_type.is_none() {
        return Err(ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: format!("structured-diff/node: id has no body: {}", q.node_id),
        });
    }

    let cfg = state.config.load();
    let store_root = cfg.store_root.clone();

    // Open + read whichever side(s) we actually need. Both sides are
    // independent — try_join.
    let want_base = base_type.is_some();
    let want_target = target_type.is_some();
    let base_fut = async {
        if want_base {
            let snap = Arc::new(
                state
                    .open_manifest(appid, depot_id, manifest_id, &q.branch)
                    .await?,
            );
            Ok::<_, ApiError>(Some(dll_side_bytes(&snap, &q.path).await?))
        } else {
            Ok(None)
        }
    };
    let target_fut = async {
        if want_target {
            let snap = Arc::new(
                state
                    .open_manifest(
                        appid,
                        q.target_depot_id,
                        q.target_manifest_id,
                        &q.target_branch,
                    )
                    .await?,
            );
            Ok::<_, ApiError>(Some(dll_side_bytes(&snap, &q.path).await?))
        } else {
            Ok(None)
        }
    };
    let (base_side, target_side) = tokio::try_join!(base_fut, target_fut)?;

    let base_text = match (base_type, base_side.as_ref()) {
        (Some(t), Some((bytes, sha))) => Some(
            crate::dll::decompile_type(&store_root, sha, bytes, t)
                .await
                .map_err(|e| ApiError {
                    status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    message: e.to_string(),
                })?,
        ),
        _ => None,
    };
    let target_text = match (target_type, target_side.as_ref()) {
        (Some(t), Some((bytes, sha))) => Some(
            crate::dll::decompile_type(&store_root, sha, bytes, t)
                .await
                .map_err(|e| ApiError {
                    status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    message: e.to_string(),
                })?,
        ),
        _ => None,
    };

    let body = match (base_text, target_text) {
        (Some(b), Some(t)) => {
            let text = crate::unity::dump_value::dump_object_json_unified_diff(&b, &t);
            (
                [(
                    header::CONTENT_TYPE,
                    "text/x-diff; charset=utf-8".to_string(),
                )],
                text,
            )
                .into_response()
        }
        (Some(b), None) => (
            [(
                header::CONTENT_TYPE,
                "text/x-csharp; charset=utf-8".to_string(),
            )],
            b,
        )
            .into_response(),
        (None, Some(t)) => (
            [(
                header::CONTENT_TYPE,
                "text/x-csharp; charset=utf-8".to_string(),
            )],
            t,
        )
            .into_response(),
        (None, None) => {
            return Err(ApiError {
                status: axum::http::StatusCode::BAD_REQUEST,
                message: "structured-diff/node could not resolve either side".to_string(),
            });
        }
    };
    Ok((crate::http::ImmutableCache, body))
}

/// Query string for the per-node structured-diff content endpoint.
/// Frontend passes the raw tree-row id; the endpoint parses out which
/// sides to dump and from where based on the id's prefix shape (see
/// [`split_diff_id`]).
#[derive(Deserialize, utoipa::IntoParams)]
pub struct StructuredDiffNodeQuery {
    pub path: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    pub target_depot_id: DepotId,
    pub target_manifest_id: ManifestId,
    #[serde(default = "default_branch")]
    pub target_branch: String,
    /// Tree-row id from the diff tree. One of:
    /// - `<inner>` — matched, same id on both sides → dump both with
    ///   `<inner>` on each.
    /// - `mod:<base-inner>,<target-inner>` — matched but renumbered.
    /// - `base:<inner>` — added (only on base).
    /// - `target:<inner>` — removed (only on target).
    pub node_id: String,
}

/// Resolves a diff-tree id into the per-side inner ids the format
/// dump helpers want. Returns `(base, target)` — either side `None`
/// means "don't dump this side".
fn split_diff_id(node_id: &str) -> (Option<&str>, Option<&str>) {
    if let Some(rest) = node_id.strip_prefix("base:") {
        return (Some(rest), None);
    }
    if let Some(rest) = node_id.strip_prefix("target:") {
        return (None, Some(rest));
    }
    if let Some(rest) = node_id.strip_prefix("mod:")
        && let Some((b, t)) = rest.split_once(',')
    {
        return (Some(b), Some(t));
    }
    // No prefix — matched node with same id on both sides.
    (Some(node_id), Some(node_id))
}

/// Per-node body for a structured diff entry. Dumps both sides as
/// JSON, then runs the same unified-diff used by `/file/diff` so
/// the frontend can render with `lang="diff"` and get colouring for
/// free. When only one side has a path-id the body returns that
/// side's JSON unchanged (no `+`/`-` decorations) so an "added" or
/// "removed" node still shows useful content.
#[cfg(feature = "unity")]
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured-diff/node",
    params(StructuredDiffNodeQuery)
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_structured_diff_node(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<StructuredDiffNodeQuery>,
) -> Result<(crate::http::ImmutableCache, Response)> {
    use crate::transform::Transformer;

    let kind = crate::transform::tools::transformer_for(&q.path);
    match kind {
        Some(Transformer::UnitySerialized) | Some(Transformer::Dll) => {}
        _ => {
            return Err(ApiError {
                status: axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                message: format!("structured diff not supported for: {}", q.path),
            });
        }
    }

    if matches!(kind, Some(Transformer::Dll)) {
        return dll_node_body(&state, appid, depot_id, manifest_id, &q).await;
    }

    // Split the diff-tree id into per-side inner ids, then parse the
    // unity-specific `obj:<pathid>` shape. Other inner shapes (section
    // headers, class-stats rows) have no per-object content and bail
    // out with `None` here.
    fn parse_obj_id(node_id: &str) -> Option<i64> {
        crate::unity::tree::parse_object_node_id(node_id)
    }
    let (base_inner, target_inner) = split_diff_id(&q.node_id);
    let base_pid = base_inner.and_then(parse_obj_id);
    let target_pid = target_inner.and_then(parse_obj_id);

    // Two snapshots in parallel (skip whichever side has no id).
    let base_snap = if base_pid.is_some() {
        Some(
            state
                .open_manifest(appid, depot_id, manifest_id, &q.branch)
                .await?,
        )
    } else {
        None
    };
    let target_snap = if target_pid.is_some() {
        Some(
            state
                .open_manifest(
                    appid,
                    q.target_depot_id,
                    q.target_manifest_id,
                    &q.target_branch,
                )
                .await?,
        )
    } else {
        None
    };

    let path = q.path.clone();
    let base_arc = base_snap.map(Arc::new);
    let target_arc = target_snap.map(Arc::new);

    let (base_text, target_text) = tokio::task::spawn_blocking(move || {
        let b = base_arc
            .zip(base_pid)
            .map(|(snap, pid)| crate::unity::dump_value::dump_object_json(snap, &path, pid))
            .transpose();
        let t = target_arc
            .zip(target_pid)
            .map(|(snap, pid)| crate::unity::dump_value::dump_object_json(snap, &path, pid))
            .transpose();
        (b, t)
    })
    .await
    .map_err(|e| ApiError {
        status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("structured-diff/node task panicked: {e}"),
    })?;

    let base_text = base_text.map_err(|e| ApiError {
        status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        message: e.to_string(),
    })?;
    let target_text = target_text.map_err(|e| ApiError {
        status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        message: e.to_string(),
    })?;

    // One-sided node → return the available JSON verbatim. Both-sided
    // → unified diff so the frontend can highlight with `lang="diff"`.
    let body = match (base_text, target_text) {
        (Some(b), Some(t)) => {
            let text = crate::unity::dump_value::dump_object_json_unified_diff(&b, &t);
            (
                [(
                    header::CONTENT_TYPE,
                    "text/x-diff; charset=utf-8".to_string(),
                )],
                text,
            )
                .into_response()
        }
        (Some(b), None) => (
            [(
                header::CONTENT_TYPE,
                "application/json; charset=utf-8".to_string(),
            )],
            b,
        )
            .into_response(),
        (None, Some(t)) => (
            [(
                header::CONTENT_TYPE,
                "application/json; charset=utf-8".to_string(),
            )],
            t,
        )
            .into_response(),
        (None, None) => {
            return Err(ApiError {
                status: axum::http::StatusCode::BAD_REQUEST,
                message: "structured-diff/node ids did not parse to object ids".to_string(),
            });
        }
    };
    Ok((crate::http::ImmutableCache, body))
}

/// Open a manifest, find the file, enqueue its chunks for download
/// and wait. Returns the (Arc'd) snapshot, ready to be handed to a
/// blocking task that needs `chunk_store` access.
#[cfg(feature = "unity")]
async fn prepare_structured_side(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    path: &str,
    branch: &str,
) -> Result<Arc<crate::state::Snapshot>> {
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
            .ok_or_else(|| ApiError {
                status: axum::http::StatusCode::NOT_FOUND,
                message: format!("file not in manifest {depot_id}/{manifest_id}: {path}"),
            })?;
        file.chunks
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
