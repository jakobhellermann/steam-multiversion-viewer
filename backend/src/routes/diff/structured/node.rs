// TODO(ai-review): review for style and correctness
//! Per-node content for the structured diff tree, fetched lazily per row.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use serde::Deserialize;

use crate::http::ApiError;
use crate::state::{AppState, Snapshot};
use crate::steam::{AppId, DepotId, ManifestId};

use super::dll_side_bytes;
use crate::routes::Result;
use crate::routes::diff::{default_branch, diff_label};

/// Query string for the per-node structured-diff content endpoint; `node_id` is the raw tree-row id, parsed into per-side dump targets by [`split_diff_id`]'s prefix shape.
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

/// Structured tree diff node content
///
/// Dumps both sides as JSON and runs the same unified-diff as `/file/diff`. When only one side has a path-id, returns that side's JSON unchanged (no `+`/`-` decorations) for an added/removed node.
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

    state.steam()?; // 401 if not logged in
    let kind = transform::tools::transformer_for(&q.path, None);
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

/// Per-node body for the Dll route: `base_id`/`target_id` are `type:<FQN>` ids from [`transform::dll::diff::build_tree`]; decompiles via `ilspycmd -t` (cached) and returns a unified diff, or the lone side verbatim for added/removed.
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

    let (base_side, target_side) = tokio::try_join!(
        open_dll_side(
            state,
            appid,
            depot_id,
            manifest_id,
            &q.branch,
            &q.path,
            base_type.is_some()
        ),
        open_dll_side(
            state,
            appid,
            q.target_depot_id,
            q.target_manifest_id,
            &q.target_branch,
            &q.path,
            target_type.is_some(),
        ),
    )?;

    let base_text = match (base_type, base_side.as_ref()) {
        (Some(t), Some(side)) => Some(Dumped {
            mime: MIME_CSHARP,
            text: transform::dll::decompile_type(&store_root, &side.sha, &side.bytes, t)
                .await
                .map_err(ApiError::from_transform)?,
        }),
        _ => None,
    };
    let target_text = match (target_type, target_side.as_ref()) {
        (Some(t), Some(side)) => Some(Dumped {
            mime: MIME_CSHARP,
            text: transform::dll::decompile_type(&store_root, &side.sha, &side.bytes, t)
                .await
                .map_err(ApiError::from_transform)?,
        }),
        _ => None,
    };

    let creation_time = |side: &Option<DllSide>| side.as_ref().map_or(0, |s| s.creation_time);
    let base_label = diff_label(depot_id, manifest_id, creation_time(&base_side));
    let target_label = diff_label(
        q.target_depot_id,
        q.target_manifest_id,
        creation_time(&target_side),
    );
    let body = node_body_response(base_text, target_text, &base_label, &target_label).ok_or_else(
        || ApiError::bad_request("structured-diff/node could not resolve either side"),
    )?;
    Ok((crate::http::ImmutableCache, body))
}

/// Per-node body for the UnityBundle route. Bundle node ids carry an `archive:<entry>/` prefix wrapping the [`split_diff_id`] shapes (`base:`/`target:`/`mod:`); stripping both gives a plain `obj:<pid>` for [`transform::unity::serializedfile::dump_value::dump_bundle_object_json`].
///
/// One-sided (fully Added/Removed) rows have no `base:`/`target:` wrapper, so both sides are tried speculatively; the expected missing-side error is swallowed rather than failing the request.
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

    let base_snap = open_node_snapshot(
        state,
        appid,
        depot_id,
        manifest_id,
        &q.branch,
        base_target.is_some(),
    )
    .await?;
    let target_snap = open_node_snapshot(
        state,
        appid,
        q.target_depot_id,
        q.target_manifest_id,
        &q.target_branch,
        target_target.is_some(),
    )
    .await?;

    let base_ct = base_snap
        .as_ref()
        .map(|s| s.manifest().creation_time)
        .unwrap_or(0);
    let target_ct = target_snap
        .as_ref()
        .map(|s| s.manifest().creation_time)
        .unwrap_or(0);
    let bundle_path = q.path.clone();

    let base_side = base_snap
        .as_ref()
        .map(|s| unity_side_with_scratch(state, appid, depot_id, manifest_id, &q.branch, s.clone()))
        .transpose()?;
    let target_side = target_snap
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

    // PPtr markers are side-agnostic; per-line side comes from the unified-diff `+`/`-` gutter.
    let (base_text, target_text) = tokio::task::spawn_blocking(move || {
        let dump_side =
            |side: Option<(Arc<crate::state::manifest_cache::ManifestScratch>, String)>,
             target: Option<(String, rabex_env::rabex::objects::pptr::PathId)>|
             -> Option<anyhow::Result<Dumped>> {
                side.zip(target).map(|((scratch, data_dir), (entry, pid))| {
                    let unity = scratch
                        .unity_already_initialized()
                        .expect("unity scratch was initialised on the async side");
                    let env = &unity.env;
                    let opts = transform::unity::serializedfile::dump_value::DumpOptions {
                        spp_key: unity.secure_player_prefs_key(),
                        playmaker_game: Some(unity),
                    };
                    let (mime, text) =
                        transform::unity::serializedfile::dump_value::dump_bundle_object_json(
                            env,
                            &data_dir,
                            &bundle_path,
                            &entry,
                            pid,
                            opts,
                        )?;
                    Ok(Dumped { mime, text })
                })
            };
        (
            dump_side(base_side, base_target),
            dump_side(target_side, target_target),
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("structured-diff/node task panicked: {e}")))?;

    // Per-side errors degrade to "no content" rather than failing the request: a one-sided row has no wrapper, so both sides were tried speculatively and one is expected to error.
    let base_text = base_text.and_then(|r| r.ok());
    let target_text = target_text.and_then(|r| r.ok());

    let base_label = diff_label(depot_id, manifest_id, base_ct);
    let target_label = diff_label(q.target_depot_id, q.target_manifest_id, target_ct);
    let body = match node_body_response(base_text, target_text, &base_label, &target_label) {
        Some(body) => body,
        None => {
            return Err(ApiError::bad_request(format!(
                "bundle structured-diff/node could not resolve any side: {}",
                q.node_id
            )));
        }
    };
    Ok((crate::http::ImmutableCache, body))
}

#[cfg(feature = "unity")]
async fn unity_serialized_node_body(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    q: &StructuredDiffNodeQuery,
) -> Result<(crate::http::ImmutableCache, Response)> {
    // Split into per-side inner ids, then parse the unity `obj:<pathid>` shape; other shapes (section headers, class-stats rows) have no per-object content.
    fn parse_obj_id(node_id: &str) -> Option<i64> {
        transform::unity::serializedfile::tree::parse_object_node_id(node_id)
    }
    let (base_inner, target_inner) = split_diff_id(&q.node_id);
    let base_pid = base_inner.and_then(parse_obj_id);
    let target_pid = target_inner.and_then(parse_obj_id);

    let base_snap = open_node_snapshot(
        state,
        appid,
        depot_id,
        manifest_id,
        &q.branch,
        base_pid.is_some(),
    )
    .await?;
    let target_snap = open_node_snapshot(
        state,
        appid,
        q.target_depot_id,
        q.target_manifest_id,
        &q.target_branch,
        target_pid.is_some(),
    )
    .await?;

    let path = q.path.clone();
    let base_ct = base_snap
        .as_ref()
        .map(|s| s.manifest().creation_time)
        .unwrap_or(0);
    let target_ct = target_snap
        .as_ref()
        .map(|s| s.manifest().creation_time)
        .unwrap_or(0);

    let base_side = base_snap
        .as_ref()
        .map(|s| unity_side_with_scratch(state, appid, depot_id, manifest_id, &q.branch, s.clone()))
        .transpose()?;
    let target_side = target_snap
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
             -> Option<anyhow::Result<Dumped>> {
                side.zip(pid).map(|((scratch, data_dir), pid)| {
                    let unity = scratch
                        .unity_already_initialized()
                        .expect("unity scratch was initialised on the async side");
                    let opts = transform::unity::serializedfile::dump_value::DumpOptions {
                        spp_key: unity.secure_player_prefs_key(),
                        playmaker_game: Some(unity),
                    };
                    let (mime, text) =
                        transform::unity::serializedfile::dump_value::dump_object_json(
                            &unity.env, &data_dir, &path, pid, opts,
                        )?;
                    Ok(Dumped { mime, text })
                })
            };
        (
            dump_side(base_side, base_pid),
            dump_side(target_side, target_pid),
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("structured-diff/node task panicked: {e}")))?;

    let base_text = base_text
        .transpose()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let target_text = target_text
        .transpose()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let base_label = diff_label(depot_id, manifest_id, base_ct);
    let target_label = diff_label(q.target_depot_id, q.target_manifest_id, target_ct);
    let body = match node_body_response(base_text, target_text, &base_label, &target_label) {
        Some(body) => body,
        None => {
            return Err(ApiError::bad_request(
                "structured-diff/node ids did not parse to object ids",
            ));
        }
    };
    Ok((crate::http::ImmutableCache, body))
}

/// MIME type of a decompiled C# body.
const MIME_CSHARP: &str = "text/x-csharp";

/// One side of a Dll node: assembly bytes, the sha the decompile cache is keyed on, and the manifest's creation time.
struct DllSide {
    bytes: Vec<u8>,
    sha: [u8; 20],
    creation_time: u32,
}

/// One side's rendered body plus its MIME; the MIME must reach the response or a one-sided node gets mislabelled.
#[derive(Debug, PartialEq)]
struct Dumped {
    mime: &'static str,
    text: String,
}

/// Build the per-node body: a unified diff when both sides are present, the lone side verbatim otherwise, paired with its content type; `None` if neither resolved.
fn node_body_response(
    base: Option<Dumped>,
    target: Option<Dumped>,
    base_label: &str,
    target_label: &str,
) -> Option<Response> {
    let (content_type, text) = node_body(base, target, base_label, target_label)?;
    Some(([(header::CONTENT_TYPE, content_type)], text).into_response())
}

fn node_body(
    base: Option<Dumped>,
    target: Option<Dumped>,
    base_label: &str,
    target_label: &str,
) -> Option<(String, String)> {
    let (mime, text) = match (base, target) {
        (Some(b), Some(t)) => (
            "text/x-diff",
            transform::diff::unified_diff_text(&b.text, &t.text, base_label, target_label),
        ),
        (Some(one), None) | (None, Some(one)) => (one.mime, one.text),
        (None, None) => return None,
    };
    Some((format!("{mime}; charset=utf-8"), text))
}

/// Resolve a bundle diff-tree node id to per-side `(archive_entry, obj_pid)` tuples: strips the outer `archive:<entry>/` prefix, then [`split_diff_id`]s and parses the object path-id on each side.
/// `None` for a side with no parseable object id (section headers, class-stats rows, malformed input).
#[cfg(feature = "unity")]
type ArchiveObjectRef = (String, i64);

#[cfg(feature = "unity")]
fn parse_bundle_node_id(node_id: &str) -> (Option<ArchiveObjectRef>, Option<ArchiveObjectRef>) {
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

/// Resolves a diff-tree id into per-side inner ids; `None` for a side means don't dump it.
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
    // No prefix: matched node with same id on both sides.
    (Some(node_id), Some(node_id))
}

/// Open one side's manifest and read its assembly bytes + sha, or `None` if not needed.
async fn open_dll_side(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    path: &str,
    needed: bool,
) -> Result<Option<DllSide>> {
    let Some(snap) =
        open_node_snapshot(state, appid, depot_id, manifest_id, branch, needed).await?
    else {
        return Ok(None);
    };
    let creation_time = snap.manifest().creation_time;
    let (bytes, sha) = dll_side_bytes(&snap, path).await?;
    Ok(Some(DllSide {
        bytes,
        sha,
        creation_time,
    }))
}

/// Open one side's manifest for a node-body request, or `None` if that side isn't needed.
async fn open_node_snapshot(
    state: &AppState,
    appid: AppId,
    depot_id: DepotId,
    manifest_id: ManifestId,
    branch: &str,
    needed: bool,
) -> Result<Option<Arc<Snapshot>>> {
    if !needed {
        return Ok(None);
    }
    Ok(Some(Arc::new(
        state
            .open_manifest(appid, depot_id, manifest_id, branch)
            .await?,
    )))
}

/// Resolve the per-manifest unity `Environment` + data_dir — see
/// [`crate::routes::unity::scratch_side`]; 415s if the manifest isn't a
/// unity game.
#[cfg(feature = "unity")]
use crate::routes::unity::scratch_side as unity_side_with_scratch;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_body_keeps_the_one_sided_mime() {
        let one = Dumped {
            mime: "text/x-playmaker-fsm",
            text: "state Idle\n".to_string(),
        };
        assert_eq!(
            node_body(Some(one), None, "base", "target"),
            Some((
                "text/x-playmaker-fsm; charset=utf-8".to_string(),
                "state Idle\n".to_string()
            ))
        );
    }

    #[test]
    fn node_body_diffs_both_sides() {
        let base = Dumped {
            mime: "application/json",
            text: "a\n".to_string(),
        };
        let target = Dumped {
            mime: "application/json",
            text: "b\n".to_string(),
        };
        assert_eq!(
            node_body(Some(base), Some(target), "base", "target"),
            Some((
                "text/x-diff; charset=utf-8".to_string(),
                "--- target\n+++ base\n@@ -1 +1 @@\n-b\n+a\n".to_string()
            ))
        );
    }

    #[test]
    fn node_body_is_none_without_a_resolved_side() {
        assert_eq!(node_body(None, None, "base", "target"), None);
    }

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
    #[cfg(feature = "unity")]
    #[test]
    fn parse_bundle_node_id_matched_pair() {
        let (base, target) = parse_bundle_node_id("archive:CAB-abc/obj:42");
        assert_eq!(base, Some(("CAB-abc".to_string(), 42)));
        assert_eq!(target, Some(("CAB-abc".to_string(), 42)));
    }

    /// One-sided row inside a matched archive subtree: the side marker lives *inside* the archive prefix.
    /// Regression test for the "id has no body" bug where the outer split saw no side prefix against `archive:CAB-.../target:obj:335`.
    #[cfg(feature = "unity")]
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

    #[cfg(feature = "unity")]
    #[test]
    fn parse_bundle_node_id_one_sided_base() {
        let (base, target) = parse_bundle_node_id("archive:CAB-abc/base:obj:7");
        assert_eq!(base, Some(("CAB-abc".to_string(), 7)));
        assert_eq!(target, None);
    }

    /// Renumbered object: same logical asset on both sides but its
    /// path-id changed between manifests.
    #[cfg(feature = "unity")]
    #[test]
    fn parse_bundle_node_id_mod_pair() {
        let (base, target) = parse_bundle_node_id("archive:CAB-abc/mod:obj:10,obj:11");
        assert_eq!(base, Some(("CAB-abc".to_string(), 10)));
        assert_eq!(target, Some(("CAB-abc".to_string(), 11)));
    }

    /// Section header / class-stats row inside an archive: no per-object body to dump.
    #[cfg(feature = "unity")]
    #[test]
    fn parse_bundle_node_id_non_object_inner() {
        let (base, target) = parse_bundle_node_id("archive:CAB-abc/section:hierarchy");
        assert_eq!(base, None);
        assert_eq!(target, None);
    }

    /// Ids that don't even carry the archive prefix don't belong in this endpoint at all.
    #[cfg(feature = "unity")]
    #[test]
    fn parse_bundle_node_id_no_archive_prefix() {
        let (base, target) = parse_bundle_node_id("obj:42");
        assert_eq!(base, None);
        assert_eq!(target, None);
    }
}
