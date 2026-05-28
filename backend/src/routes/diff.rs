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
#[allow(unused_imports)]
use serde_json::json;
use steam_vent_depot::{DepotFile, FileKind};
use tokio::sync::Semaphore;
use utoipa::ToSchema;

use crate::http::ApiError;
use crate::state::AppState;
#[cfg(feature = "unity")]
use crate::state::manifest_cache::Environment;
use crate::steam::{AppId, DepotId, ManifestId};

use super::Result;
use super::files::FileViewQuery;
use super::library::ManifestRef;

#[derive(Deserialize, ToSchema)]
pub struct ManifestDiffRequest {
    /// The manifest whose paths we report. A path is included if it is
    /// content-changed against any `other`, or absent from every
    /// `other` (added in `base`).
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "entries": [
        {"path": "Data/Engine.dll",      "status": "changed"},
        {"path": "Data/NewModule.dll",   "status": "added"}
    ]
}))]
pub struct ManifestDiffResponse {
    /// Entries are paths *in `base`* that are either content-different
    /// from at least one `other` (`Changed`) or absent from every
    /// `other` (`Added`). Paths only present in some `other` but
    /// missing from `base` are deliberately not reported — the file
    /// tree is rooted in `base` and has no row to attach them to.
    pub entries: Vec<ManifestDiffEntry>,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestDiffEntry {
    pub path: String,
    pub status: ManifestDiffStatus,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ManifestDiffStatus {
    /// In `base`, in *no* `other`.
    Added,
    /// In `base` and in at least one `other`, with a different content
    /// fingerprint somewhere. Wins over `Added` when both apply across
    /// multiple targets.
    Changed,
}

/// Manifest path-level diff
///
/// Each reported entry is a path in `base` whose status is either
/// `Changed` (content differs from some `other`) or `Added` (absent
/// from every `other`). Identity is `(kind, size, sha, linktarget)`.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/diff",
    tag = "diff",
    request_body = ManifestDiffRequest,
    responses((status = 200, body = ManifestDiffResponse))
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

    // Two passes per `other`:
    //  1. For each base path: is it absent here? → bump toward `Added`.
    //  2. For each path the `other` *and* base share: do fingerprints
    //     match? If not → mark `Changed`.
    // `Changed` wins over `Added` across multiple `others` because it
    // is the strictly more informative label: a path that is "added in
    // base vs target A" but "changed vs target B" is, against the
    // aggregate, still demonstrably different in *content* somewhere.
    let mut absent_in_all: HashSet<&str> = base_by_path.keys().copied().collect();
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
            if let Some(base_f) = base_by_path.get(f.path.as_str())
                && fp(base_f) != fp(f)
            {
                changed.insert(f.path.clone());
            }
        }
        // Any base path this `other` covers (matching or not) disqualifies
        // it from being "absent in *all* others", which is our Added test.
        absent_in_all.retain(|p| !other_paths.contains(p));
    }

    let mut entries: Vec<ManifestDiffEntry> =
        Vec::with_capacity(changed.len() + absent_in_all.len());
    for path in &changed {
        entries.push(ManifestDiffEntry {
            path: path.clone(),
            status: ManifestDiffStatus::Changed,
        });
    }
    for path in &absent_in_all {
        // Changed wins — skip if the same path also showed up as changed
        // against some other target.
        if changed.contains(*path) {
            continue;
        }
        entries.push(ManifestDiffEntry {
            path: (*path).to_owned(),
            status: ManifestDiffStatus::Added,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(ManifestDiffResponse { entries }))
}

#[derive(Deserialize, ToSchema)]
pub struct FileDiffTargetsRequest {
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
    pub path: String,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "statuses": [
        {"depot_id": 1234567, "manifest_id": "9876543210987654321", "status": "different"},
        {"depot_id": 1234567, "manifest_id": "1111222233334444555",  "status": "same"},
        {"depot_id": 1234567, "manifest_id": "5555666677778888999",  "status": "missing"}
    ]
}))]
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

/// Find manifests with changes
///
/// For one file (`path`) against N target manifests, report which
/// ones have it byte-identical to `base` (`same`), present but
/// different (`different`), or absent (`missing`).
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/file/diff-targets",
    tag = "diff",
    request_body = FileDiffTargetsRequest,
    responses((status = 200, body = FileDiffTargetsResponse))
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
pub struct ManifestDiffTargetsRequest {
    pub base: ManifestRef,
    pub others: Vec<ManifestRef>,
    /// Whitespace-separated AND-tokens matched as lowercased substrings
    /// against the depot path. Empty / missing → no filtering, every
    /// path counts (in which case this endpoint degenerates to "does
    /// any base path differ from this target", almost always `true`).
    #[serde(default)]
    pub query: String,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "matching_targets": [
        {"depot_id": 1234567, "manifest_id": "9876543210987654321"},
        {"depot_id": 1234567, "manifest_id": "5555666677778888999"}
    ]
}))]
pub struct ManifestDiffTargetsResponse {
    /// Targets that have at least one path matching `query` whose
    /// content fingerprint differs from base — or which is absent on
    /// the target side. Targets with no qualifying differences are
    /// omitted entirely.
    pub matching_targets: Vec<ManifestRefShort>,
}

#[derive(Serialize, ToSchema)]
pub struct ManifestRefShort {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
}

/// Find manifests with changes in a path subset
///
/// Like `/manifests/diff` but answers a yes/no per target instead of
/// returning the path list. The frontend uses this to filter the
/// "Compare to…" dropdown to only those targets where the user's
/// current search would yield changes.
#[utoipa::path(
    post,
    path = "/api/apps/{appid}/manifests/diff-targets",
    tag = "diff",
    request_body = ManifestDiffTargetsRequest,
    responses((status = 200, body = ManifestDiffTargetsResponse))
)]
#[tracing::instrument(skip_all, fields(others = body.others.len(), query = %body.query))]
pub async fn manifest_diff_targets(
    State(state): State<AppState>,
    Path(appid): Path<AppId>,
    Json(body): Json<ManifestDiffTargetsRequest>,
) -> Result<Json<ManifestDiffTargetsResponse>> {
    let base_snap = state
        .open_manifest(
            appid,
            body.base.depot_id,
            body.base.manifest_id,
            &body.base.branch,
        )
        .await?;

    // Pre-compute the path subset we care about: lowercased AND-token
    // substring match on the depot path, mirroring the in-app search
    // box. With no tokens every base path passes.
    let tokens: Vec<String> = body
        .query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .collect();
    let base = base_snap.manifest();
    let mut base_subset: HashMap<&str, &DepotFile> = HashMap::with_capacity(base.files.len());
    for f in &base.files {
        if matches!(f.kind, FileKind::Directory) {
            continue;
        }
        if !tokens.is_empty() {
            let path_lc = f.path.to_lowercase();
            if !tokens.iter().all(|t| path_lc.contains(t)) {
                continue;
            }
        }
        base_subset.insert(f.path.as_str(), f);
    }

    // Identity tuple — same as manifest_diff.
    fn fp(f: &DepotFile) -> (FileKind, u64, Option<[u8; 20]>, Option<&str>) {
        (f.kind, f.size, f.sha, f.linktarget.as_deref())
    }

    // Open every distinct `other` in parallel. Self-comparisons skip.
    let sem = Arc::new(Semaphore::new(8));
    let mut fu = FuturesUnordered::new();
    let mut seen = HashSet::new();
    for r in &body.others {
        if !seen.insert((r.depot_id, r.manifest_id)) {
            continue;
        }
        if (r.depot_id, r.manifest_id) == (body.base.depot_id, body.base.manifest_id) {
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

    let mut matching_targets: Vec<ManifestRefShort> = Vec::new();
    while let Some((depot_id, manifest_id, result)) = fu.next().await {
        let snap = match result {
            Ok(s) => s,
            Err(err) => {
                // Same defensive policy as `file_diff_targets`: treat
                // an unfetchable target as "different" so the user
                // still sees the candidate and can investigate.
                tracing::warn!(%depot_id, %manifest_id, %err, "open_manifest failed in manifest_diff_targets");
                matching_targets.push(ManifestRefShort {
                    depot_id,
                    manifest_id,
                });
                continue;
            }
        };
        let other = snap.manifest();
        let mut other_by_path: HashMap<&str, &DepotFile> =
            HashMap::with_capacity(other.files.len());
        for f in &other.files {
            if matches!(f.kind, FileKind::Directory) {
                continue;
            }
            other_by_path.insert(f.path.as_str(), f);
        }
        let mut has_diff = false;
        for (path, base_f) in &base_subset {
            match other_by_path.get(path) {
                None => {
                    has_diff = true;
                    break;
                }
                Some(other_f) => {
                    if fp(base_f) != fp(other_f) {
                        has_diff = true;
                        break;
                    }
                }
            }
        }
        if has_diff {
            matching_targets.push(ManifestRefShort {
                depot_id,
                manifest_id,
            });
        }
    }

    Ok(Json(ManifestDiffTargetsResponse { matching_targets }))
}

#[derive(Deserialize, ToSchema)]
pub struct FileDiffRequest {
    pub path: String,
    pub target: ManifestRef,
}

/// Unified file diff
///
/// Both sides go through the same text-resolution as `/file/transformed`
/// — if a transformer is registered, the diff is on its output.
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
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "file not in manifest {depot_id}/{manifest_id}: {path}"
                ))
            })?;
        let sha = file
            .sha
            .ok_or_else(|| ApiError::bad_request(format!("file has no content sha: {path}")))?;
        (
            file.path.clone(),
            sha,
            file.chunks
                .iter()
                .map(|c| (c.sha, u64::from(c.size_compressed)))
                .collect::<Vec<_>>(),
        )
    };

    if let Some(transformer) = transform::tools::transformer_for(&file_path) {
        let cfg = state.config.load();
        if let Some(cached) = transform::read_cached(&cfg.store_root, &file_sha)? {
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
            transform::Transformer::Cli(tool) => {
                let bytes = snapshot.read_full(&file_path).await?;
                transform::run_and_cache(&cfg.store_root, tool, &file_sha, &bytes)
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))?
            }
            #[cfg(feature = "unity")]
            transform::Transformer::UnitySerialized => {
                let (env, data_dir) = unity_side(
                    state,
                    appid,
                    depot_id,
                    manifest_id,
                    branch,
                    snapshot.clone(),
                )?;
                let file_path = file_path.clone();
                tokio::task::spawn_blocking(move || {
                    transform::unity::dump_unity_serialized(&env, &data_dir, &file_path)
                })
                .await
                .map_err(|e| ApiError::internal(format!("unity dump task panicked: {e}")))?
                .map_err(|e| ApiError::internal(e.to_string()))?
            }
            transform::Transformer::Dll => {
                // .NET assemblies don't have a single text dump to
                // diff — they're inherently per-type. A future
                // structured-diff endpoint can cover this; for now
                // the file falls through to the byte-equality view.
                return Err(ApiError::unsupported_media_type(
                    ".NET assemblies have no text-diff representation yet",
                ));
            }
            #[cfg(feature = "unity")]
            transform::Transformer::UnityBundle => {
                // Bundles contain multiple SerializedFiles; a single
                // text dump for diff is awkward and the structured
                // tree carries the actual signal. Punt for now.
                return Err(ApiError::unsupported_media_type(
                    "Unity bundles have no text-diff representation yet",
                ));
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
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
        ApiError::unsupported_media_type(format!(
            "file is binary and has no registered transformer: {file_path}"
        ))
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

/// Structured tree diff
///
/// Returns a pruned tree where each node is tagged
/// `added`/`removed`/`changed`/`unchanged`; unchanged leaves are
/// dropped so the payload carries only the spine to every difference.
///
/// Per-node content (the actual JSON dump for changed objects) is
/// fetched lazily by the client via the existing
/// `/file/structured/node` endpoint, once per side.
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
) -> Result<(
    crate::http::ImmutableCache,
    Json<transform::structured::StructuredTree>,
)> {
    use transform::Transformer;

    let kind = transform::tools::transformer_for(&q.path);

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
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized) => {
            let path = q.path.clone();
            let (base_env, base_data_dir) = unity_side(
                &state,
                appid,
                depot_id,
                manifest_id,
                &q.branch,
                base.clone(),
            )?;
            let (target_env, target_data_dir) = unity_side(
                &state,
                appid,
                q.target_depot_id,
                q.target_manifest_id,
                &q.target_branch,
                target.clone(),
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
            .await
            .map_err(|e| ApiError::internal(format!("structured-diff task panicked: {e}")))?
            .map_err(|e| ApiError::internal(e.to_string()))?
        }
        #[cfg(feature = "unity")]
        Some(Transformer::UnityBundle) => {
            use rabex_env::resolver::EnvResolver;
            let path = q.path.clone();
            let (base_env, base_data_dir) = unity_side(
                &state,
                appid,
                depot_id,
                manifest_id,
                &q.branch,
                base.clone(),
            )?;
            let (target_env, target_data_dir) = unity_side(
                &state,
                appid,
                q.target_depot_id,
                q.target_manifest_id,
                &q.target_branch,
                target.clone(),
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
            .await
            .map_err(|e| ApiError::internal(format!("structured-diff task panicked: {e}")))?
            .map_err(|e| ApiError::internal(e.to_string()))?
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
            let tree = transform::dll::diff::build_tree(
                &store_root,
                &from_sha,
                &from_bytes,
                &to_sha,
                &to_bytes,
                &q.path,
            )
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
            // Kick `ilspycmd -p` for both DLLs in the background.
            // Per-type lazy decompile calls from the node endpoint then
            // hit the cache instead of spawning a fresh `ilspycmd -t`.
            // Idempotent: skipped if already warming this sha.
            transform::dll::warm_full_decompile(&store_root, from_sha, from_bytes);
            transform::dll::warm_full_decompile(&store_root, to_sha, to_bytes);
            tree
        }
        _ => {
            return Err(ApiError::unsupported_media_type(format!(
                "structured diff not supported for: {}",
                q.path
            )));
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
        .ok_or_else(|| ApiError::bad_request(format!("file has no content sha: {path}")))?;
    let bytes = snap.read_full(path).await?.to_vec();
    Ok((bytes, sha))
}

/// Per-node body for the UnityBundle structured-diff route. Bundle
/// node ids carry an `archive:<entry>/` prefix; the inner part is
/// either bare (`obj:<pid>` — matched both sides), or wrapped in the
/// same `base:` / `target:` / `mod:` shapes that [`split_diff_id`]
/// handles. We split that first, then strip `archive:<entry>/` from
/// each per-side id to land back at a plain `obj:<pid>` we can dump
/// with [`transform::unity::serializedfile::dump_value::dump_bundle_object_json`].
///
/// One-sided rows from a fully-Added or fully-Removed archive entry
/// have no `base:`/`target:` wrapper (the prefix-id pass inside
/// [`transform::unity::bundle::one_sided_entry`] doesn't add one) — there
/// we fall back to attempting both sides and returning whichever has
/// the object. The missing-side errors get swallowed silently rather
/// than 500ing the request.
#[cfg(feature = "unity")]
async fn bundle_node_body(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    q: &StructuredDiffNodeQuery,
) -> Result<(crate::http::ImmutableCache, Response)> {
    let (base_target, target_target) = parse_bundle_node_id(&q.node_id);
    if base_target.is_none() && target_target.is_none() {
        return Err(ApiError::bad_request(format!(
            "bundle structured-diff/node: id has no body: {}",
            q.node_id
        )));
    }

    // Two snapshots in parallel — skip whichever side isn't needed.
    let base_snap = if base_target.is_some() {
        Some(
            state
                .open_manifest(appid, depot_id, manifest_id, &q.branch)
                .await?,
        )
    } else {
        None
    };
    let target_snap = if target_target.is_some() {
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

    let base_ct = base_snap
        .as_ref()
        .map(|s| s.manifest().creation_time)
        .unwrap_or(0);
    let target_ct = target_snap
        .as_ref()
        .map(|s| s.manifest().creation_time)
        .unwrap_or(0);
    let base_arc = base_snap.map(Arc::new);
    let target_arc = target_snap.map(Arc::new);
    let bundle_path = q.path.clone();

    let base_side = base_arc
        .as_ref()
        .map(|s| unity_side_with_scratch(state, appid, depot_id, manifest_id, &q.branch, s.clone()))
        .transpose()?;
    let target_side = target_arc
        .as_ref()
        .map(|s| {
            unity_side_with_scratch(
                state,
                appid,
                q.target_depot_id,
                q.target_manifest_id,
                &q.target_branch,
                s.clone(),
            )
        })
        .transpose()?;

    // Dump both sides — PPtr markers are side-agnostic, the frontend
    // recovers per-line side from the unified-diff `+`/`-` gutter.
    let (base_text, target_text) = tokio::task::spawn_blocking(move || {
        use rabex_env::resolver::EnvResolver;
        let dump_side =
            |side: Option<(Arc<crate::state::manifest_cache::ManifestScratch>, String)>,
             target: Option<(String, rabex_env::rabex::objects::pptr::PathId)>|
             -> Option<anyhow::Result<String>> {
                let ((scratch, data_dir), (entry, pid)) = side.zip(target)?;
                Some((|| -> anyhow::Result<String> {
                    let unity = scratch
                        .unity_already_initialized()
                        .expect("unity scratch was initialised on the async side");
                    let env = &unity.env;
                    let relative = bundle_path
                        .strip_prefix(&format!("{data_dir}/"))
                        .unwrap_or(&bundle_path);
                    let bundle_bytes = env.game_files.read_path(std::path::Path::new(relative))?;
                    let opts = transform::unity::serializedfile::dump_value::DumpOptions {
                        spp_key: unity.secure_player_prefs_key(),
                    };
                    let (_mime, text) =
                        transform::unity::serializedfile::dump_value::dump_bundle_object_json(
                            env,
                            &data_dir,
                            bundle_bytes,
                            &entry,
                            pid,
                            opts,
                        )?;
                    Ok(text)
                })())
            };
        let b = dump_side(base_side, base_target);
        let t = dump_side(target_side, target_target);
        (b, t)
    })
    .await
    .map_err(|e| ApiError::internal(format!("structured-diff/node task panicked: {e}")))?;

    // Per-side errors degrade to "no content for this side" rather
    // than failing the whole request — a one-sided Added/Removed
    // subtree row has no `base:`/`target:` wrapper, so we tried both
    // sides speculatively above and one of them is expected to error
    // with "entry not in bundle" or "object not in entry".
    let base_text = base_text.and_then(|r| r.ok());
    let target_text = target_text.and_then(|r| r.ok());

    let body = match (base_text, target_text) {
        (Some(b), Some(t)) => {
            let base_label = diff_label(depot_id, manifest_id, base_ct);
            let target_label = diff_label(q.target_depot_id, q.target_manifest_id, target_ct);
            let text = transform::diff::unified_diff_text(&b, &t, &base_label, &target_label);
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
            return Err(ApiError::bad_request(format!(
                "bundle structured-diff/node could not resolve any side: {}",
                q.node_id
            )));
        }
    };
    Ok((crate::http::ImmutableCache, body))
}

/// Per-node body for the Dll structured-diff route. `base_id` /
/// `target_id` are `type:<FQN>` ids from
/// [`transform::dll::diff::build_tree`]; we decompile the type via
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
        return Err(ApiError::bad_request(format!(
            "structured-diff/node: id has no body: {}",
            q.node_id
        )));
    }

    let cfg = state.config.load();
    let store_root = cfg.store_root.clone();

    // Open + read whichever side(s) we actually need. Both sides are
    // independent — try_join. Also stash creation_time so the diff
    // header can carry the human-readable date.
    let want_base = base_type.is_some();
    let want_target = target_type.is_some();
    let base_fut = async {
        if want_base {
            let snap = Arc::new(
                state
                    .open_manifest(appid, depot_id, manifest_id, &q.branch)
                    .await?,
            );
            let ct = snap.manifest().creation_time;
            Ok::<_, ApiError>(Some((dll_side_bytes(&snap, &q.path).await?, ct)))
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
            let ct = snap.manifest().creation_time;
            Ok::<_, ApiError>(Some((dll_side_bytes(&snap, &q.path).await?, ct)))
        } else {
            Ok(None)
        }
    };
    let (base_side, target_side) = tokio::try_join!(base_fut, target_fut)?;

    let base_text = match (base_type, base_side.as_ref()) {
        (Some(t), Some(((bytes, sha), _ct))) => Some(
            transform::dll::decompile_type(&store_root, sha, bytes, t)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?,
        ),
        _ => None,
    };
    let target_text = match (target_type, target_side.as_ref()) {
        (Some(t), Some(((bytes, sha), _ct))) => Some(
            transform::dll::decompile_type(&store_root, sha, bytes, t)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?,
        ),
        _ => None,
    };

    let body = match (base_text, target_text) {
        (Some(b), Some(t)) => {
            let base_label = diff_label(
                depot_id,
                manifest_id,
                base_side.as_ref().map(|((_, _), ct)| *ct).unwrap_or(0),
            );
            let target_label = diff_label(
                q.target_depot_id,
                q.target_manifest_id,
                target_side.as_ref().map(|((_, _), ct)| *ct).unwrap_or(0),
            );
            let text = transform::diff::unified_diff_text(&b, &t, &base_label, &target_label);
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
            return Err(ApiError::bad_request(
                "structured-diff/node could not resolve either side",
            ));
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
    /// Opaque node id from the diff tree's `id` field.
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

/// Resolve a bundle diff-tree node id to per-side `(archive_entry,
/// obj_pid)` tuples. Bundle ids carry the archive prefix on the
/// *outside* (`archive:<entry>/...`) while the `base:`/`target:`/`mod:`
/// side markers from `diff_sections` sit on the *inside*. We strip
/// the archive prefix first, [`split_diff_id`] the inner part, then
/// parse the object path-id on each per-side id.
///
/// Either side is `None` when that side has no parseable object id
/// (section headers, class-stats rows, malformed input).
#[cfg(feature = "unity")]
fn parse_bundle_node_id(node_id: &str) -> (Option<(String, i64)>, Option<(String, i64)>) {
    let Some((entry, inner)) = transform::unity::bundle::parse_archive_id(node_id) else {
        return (None, None);
    };
    let (base_inner, target_inner) = split_diff_id(inner);
    let parse_obj = |s: &str| {
        transform::unity::serializedfile::tree::parse_object_node_id(s)
            .map(|pid| (entry.to_string(), pid))
    };
    (
        base_inner.and_then(parse_obj),
        target_inner.and_then(parse_obj),
    )
}

/// Structured tree diff node content
///
/// Dumps both sides as JSON, then runs the same unified-diff used by
/// `/file/diff` so the frontend can render with `lang="diff"` and get
/// colouring for free. When only one side has a path-id the body
/// returns that side's JSON unchanged (no `+`/`-` decorations) so an
/// "added" or "removed" node still shows useful content.
#[utoipa::path(
    get,
    path = "/api/apps/{appid}/depots/{depot_id}/manifests/{manifest_id}/file/structured-diff/node",
    tag = "diff",
    params(StructuredDiffNodeQuery),
    responses(
        (
            status = 200,
            description = "Unified diff text, or one side's JSON for added/removed nodes",
            content_type = "text/x-diff",
            body = String,
        ),
        (status = 415, description = "File has no structured representation")
    )
)]
#[tracing::instrument(skip_all, fields(path = %q.path))]
pub async fn manifest_file_structured_diff_node(
    State(state): State<AppState>,
    Path((appid, depot_id, manifest_id)): Path<(AppId, DepotId, ManifestId)>,
    Query(q): Query<StructuredDiffNodeQuery>,
) -> Result<(crate::http::ImmutableCache, Response)> {
    use transform::Transformer;

    let kind = transform::tools::transformer_for(&q.path);
    let supported = match kind {
        Some(Transformer::Dll) => true,
        #[cfg(feature = "unity")]
        Some(Transformer::UnitySerialized | Transformer::UnityBundle) => true,
        _ => false,
    };
    if !supported {
        return Err(ApiError::unsupported_media_type(format!(
            "structured diff not supported for: {}",
            q.path
        )));
    }

    if matches!(kind, Some(Transformer::Dll)) {
        return dll_node_body(&state, appid, depot_id, manifest_id, &q).await;
    }

    #[cfg(feature = "unity")]
    if matches!(kind, Some(Transformer::UnityBundle)) {
        return bundle_node_body(&state, appid, depot_id, manifest_id, &q).await;
    }

    #[cfg(feature = "unity")]
    {
        return unity_serialized_node_body(&state, appid, depot_id, manifest_id, &q).await;
    }

    #[cfg(not(feature = "unity"))]
    unreachable!("non-Dll variants filtered out above when unity is disabled")
}

#[cfg(feature = "unity")]
async fn unity_serialized_node_body(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    q: &StructuredDiffNodeQuery,
) -> Result<(crate::http::ImmutableCache, Response)> {
    // Split the diff-tree id into per-side inner ids, then parse the
    // unity-specific `obj:<pathid>` shape. Other inner shapes (section
    // headers, class-stats rows) have no per-object content and bail
    // out with `None` here.
    fn parse_obj_id(node_id: &str) -> Option<i64> {
        transform::unity::serializedfile::tree::parse_object_node_id(node_id)
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
    // Stash creation_time before moving the snapshots into the
    // blocking task — used to label the diff header.
    let base_ct = base_snap
        .as_ref()
        .map(|s| s.manifest().creation_time)
        .unwrap_or(0);
    let target_ct = target_snap
        .as_ref()
        .map(|s| s.manifest().creation_time)
        .unwrap_or(0);
    let base_arc = base_snap.map(Arc::new);
    let target_arc = target_snap.map(Arc::new);

    let base_side = base_arc
        .as_ref()
        .map(|s| unity_side_with_scratch(state, appid, depot_id, manifest_id, &q.branch, s.clone()))
        .transpose()?;
    let target_side = target_arc
        .as_ref()
        .map(|s| {
            unity_side_with_scratch(
                state,
                appid,
                q.target_depot_id,
                q.target_manifest_id,
                &q.target_branch,
                s.clone(),
            )
        })
        .transpose()?;

    let (base_text, target_text) = tokio::task::spawn_blocking(move || {
        let dump_side =
            |side: Option<(Arc<crate::state::manifest_cache::ManifestScratch>, String)>,
             pid: Option<rabex_env::rabex::objects::pptr::PathId>|
             -> Option<anyhow::Result<String>> {
                let ((scratch, data_dir), pid) = side.zip(pid)?;
                Some((|| -> anyhow::Result<String> {
                    let unity = scratch
                        .unity_already_initialized()
                        .expect("unity scratch was initialised on the async side");
                    let opts = transform::unity::serializedfile::dump_value::DumpOptions {
                        spp_key: unity.secure_player_prefs_key(),
                    };
                    let (_mime, text) =
                        transform::unity::serializedfile::dump_value::dump_object_json(
                            &unity.env, &data_dir, &path, pid, opts,
                        )?;
                    Ok(text)
                })())
            };
        let b = dump_side(base_side, base_pid).transpose();
        let t = dump_side(target_side, target_pid).transpose();
        (b, t)
    })
    .await
    .map_err(|e| ApiError::internal(format!("structured-diff/node task panicked: {e}")))?;

    let base_text = base_text.map_err(|e| ApiError::internal(e.to_string()))?;
    let target_text = target_text.map_err(|e| ApiError::internal(e.to_string()))?;

    // One-sided node → return the available JSON verbatim. Both-sided
    // → unified diff so the frontend can highlight with `lang="diff"`.
    let body = match (base_text, target_text) {
        (Some(b), Some(t)) => {
            let base_label = diff_label(depot_id, manifest_id, base_ct);
            let target_label = diff_label(q.target_depot_id, q.target_manifest_id, target_ct);
            let text = transform::diff::unified_diff_text(&b, &t, &base_label, &target_label);
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
            return Err(ApiError::bad_request(
                "structured-diff/node ids did not parse to object ids",
            ));
        }
    };
    Ok((crate::http::ImmutableCache, body))
}

/// Open a manifest, find the file, enqueue its chunks for download
/// and wait. Returns the (Arc'd) snapshot, ready to be handed to a
/// blocking task that needs `chunk_store` access.
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
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "file not in manifest {depot_id}/{manifest_id}: {path}"
                ))
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

/// Resolve the per-manifest unity `Environment` + data_dir for a
/// structured-diff side. Encapsulates the scratch lookup + the 415
/// error when the manifest isn't a unity game.
#[cfg(feature = "unity")]
fn unity_side(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    snapshot: Arc<crate::state::Snapshot>,
) -> Result<(Arc<Environment>, String), ApiError> {
    let scratch = state
        .manifest_cache
        .scratch(appid, depot_id, manifest_id, branch);
    let unity = scratch
        .unity(snapshot)
        .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
    Ok((unity.env.clone(), unity.data_dir()))
}

/// Variant of [`unity_side`] that hands the whole `Arc<ManifestScratch>`
/// back. Used by the structured-diff/node path so the blocking dump
/// closure can read manifest-scoped lazy state (notably the
/// SecurePlayerPrefs key) without round-tripping through the cache
/// again.
#[cfg(feature = "unity")]
fn unity_side_with_scratch(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    snapshot: Arc<crate::state::Snapshot>,
) -> Result<(Arc<crate::state::manifest_cache::ManifestScratch>, String), ApiError> {
    let scratch = state
        .manifest_cache
        .scratch(appid, depot_id, manifest_id, branch);
    let unity = scratch
        .unity(snapshot)
        .ok_or_else(|| ApiError::unsupported_media_type("manifest is not a unity game"))?;
    let data_dir = unity.data_dir();
    Ok((scratch, data_dir))
}

#[cfg(all(test, feature = "unity"))]
mod tests {
    use super::*;

    #[test]
    fn split_diff_id_handles_side_prefixes() {
        assert_eq!(split_diff_id("obj:42"), (Some("obj:42"), Some("obj:42")));
        assert_eq!(split_diff_id("base:obj:42"), (Some("obj:42"), None));
        assert_eq!(split_diff_id("target:obj:42"), (None, Some("obj:42")));
        assert_eq!(
            split_diff_id("mod:obj:1,obj:2"),
            (Some("obj:1"), Some("obj:2"))
        );
    }

    /// Matched-pair (both sides see the same object) inside a bundle:
    /// `archive:<entry>/obj:N`. Both per-side targets resolve to the
    /// same `(entry, pid)`.
    #[test]
    fn parse_bundle_node_id_matched_pair() {
        let (base, target) = parse_bundle_node_id("archive:CAB-abc/obj:42");
        assert_eq!(base, Some(("CAB-abc".to_string(), 42)));
        assert_eq!(target, Some(("CAB-abc".to_string(), 42)));
    }

    /// One-sided row inside a matched archive subtree — the side
    /// marker lives *inside* the archive prefix (see
    /// `diff_archive_pair`'s `prefix_ids` walk). Regression test for
    /// the "id has no body" bug where the outer split was being run
    /// against `archive:CAB-…/target:obj:335` and saw no side prefix.
    #[test]
    fn parse_bundle_node_id_one_sided_target() {
        let (base, target) =
            parse_bundle_node_id("archive:CAB-d2149d4004bbc85c803fc79e72339a31/target:obj:335");
        assert_eq!(base, None);
        assert_eq!(
            target,
            Some(("CAB-d2149d4004bbc85c803fc79e72339a31".to_string(), 335))
        );
    }

    #[test]
    fn parse_bundle_node_id_one_sided_base() {
        let (base, target) = parse_bundle_node_id("archive:CAB-abc/base:obj:7");
        assert_eq!(base, Some(("CAB-abc".to_string(), 7)));
        assert_eq!(target, None);
    }

    /// Renumbered object: same logical asset on both sides but its
    /// path-id changed between manifests.
    #[test]
    fn parse_bundle_node_id_mod_pair() {
        let (base, target) = parse_bundle_node_id("archive:CAB-abc/mod:obj:10,obj:11");
        assert_eq!(base, Some(("CAB-abc".to_string(), 10)));
        assert_eq!(target, Some(("CAB-abc".to_string(), 11)));
    }

    /// Section header / class-stats row inside an archive — no
    /// per-object body to dump.
    #[test]
    fn parse_bundle_node_id_non_object_inner() {
        let (base, target) = parse_bundle_node_id("archive:CAB-abc/section:hierarchy");
        assert_eq!(base, None);
        assert_eq!(target, None);
    }

    /// Ids that don't even carry the archive prefix don't belong in
    /// this endpoint at all.
    #[test]
    fn parse_bundle_node_id_no_archive_prefix() {
        let (base, target) = parse_bundle_node_id("obj:42");
        assert_eq!(base, None);
        assert_eq!(target, None);
    }
}
