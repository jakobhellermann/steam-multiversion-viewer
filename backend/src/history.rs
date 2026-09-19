//! Format-agnostic history: files and structured nodes across a version
//! set, and the version set's own manifest-level history.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use steam_vent_depot::Manifest;
use transform::structured::{NodeId, NodeStatus};
use utoipa::ToSchema;

use crate::http::ApiError;
use crate::routes::Result;
use crate::routes::diff::manifest::{ContentId, content_id, open_manifests_concurrently};
use crate::routes::diff::structured::{
    StructuredDiffRequest, StructuredDiffSide, build_structured_diff,
};
use crate::routes::library::ManifestRef;
use crate::state::{AppState, Snapshot};
use crate::steam::AppId;

/// Manifest reference as the history endpoints take it.
pub type HistoryManifest = ManifestRef;

impl From<&HistoryManifest> for StructuredDiffSide {
    fn from(value: &HistoryManifest) -> Self {
        Self {
            depot_id: value.depot_id,
            manifest_id: value.manifest_id,
            branch: value.branch.clone(),
        }
    }
}

/// Input for a file's change history.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct FileHistoryRequest {
    pub current: HistoryManifest,
    pub previous: Vec<HistoryManifest>,
    pub path: String,
    /// Opaque id of a node in the current manifest's tree. Omit for
    /// file-level history.
    pub node_id: Option<NodeId>,
}

/// File or node state at one earlier manifest.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct FileHistoryEntry {
    #[serde(flatten)]
    pub manifest: HistoryManifest,
    pub status: HistoryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<HistoryManifest>,
    /// The corresponding node in this manifest when one was matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
    /// Opaque id in the pairwise diff tree for this history entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_node_id: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HistoryStatus {
    Initial,
    Unchanged,
    Changed,
    Added,
    Removed,
    Missing,
    Unsupported,
}

/// Build the file's history across the given manifest set, newest first.
///
/// The walk is chronological and independent of `current`: the same
/// manifest set always produces the same rows, whichever version the
/// history was opened from. `current` only anchors two things — the
/// requested `path` must exist there, and a requested `node_id` is
/// defined in its tree (the walk maps the node to every other version
/// through the pairwise diffs, up and down).
///
/// `previous` deliberately remains an input: the UI already knows the
/// app's tracked manifest set, while Steam exposes no complete history
/// API here.
pub async fn build_file_history(
    state: &AppState,
    appid: AppId,
    request: &FileHistoryRequest,
) -> Result<Vec<FileHistoryEntry>> {
    let current = Arc::new(
        state
            .open_manifest(
                appid,
                request.current.depot_id,
                request.current.manifest_id,
                &request.current.branch,
            )
            .await?,
    );
    if !current
        .manifest()
        .files
        .iter()
        .any(|f| f.path == request.path)
    {
        return Err(ApiError::not_found(format!(
            "file not in current manifest: {}",
            request.path
        )));
    }

    let mut versions = vec![(request.current.clone(), current)];
    for previous in &request.previous {
        if same_manifest(previous, &request.current) {
            continue;
        }
        let snapshot = Arc::new(
            state
                .open_manifest(
                    appid,
                    previous.depot_id,
                    previous.manifest_id,
                    &previous.branch,
                )
                .await?,
        );
        versions.push((previous.clone(), snapshot));
    }
    // Newest first, current included: the history of a file is a property
    // of the version set, not of the version it was opened from.
    versions.sort_by_key(|(_, snapshot)| Reverse(snapshot.manifest().creation_time));
    let current_index = versions
        .iter()
        .position(|(manifest, _)| same_manifest(manifest, &request.current))
        .expect("the current manifest is part of the version list");

    // Node id per version, chained outward from the current manifest
    // through the pairwise diffs. `None` = untracked (file-level walk).
    let mut node_ids: Vec<Option<NodeId>> = vec![None; versions.len()];
    node_ids[current_index] = request.node_id.clone();

    // One transition per consecutive pair (index, index + 1), computed
    // exactly once; the two passes meet at the current manifest.
    let mut outcomes: Vec<Option<PairOutcome>> = vec![None; versions.len().saturating_sub(1)];
    // Pairs above the current manifest: chain the node upward, from each
    // pair's older side to its newer one.
    for index in (0..current_index).rev() {
        let known = node_ids[index + 1].clone().map(|node| (node, Side::Older));
        let outcome = pair_transition(state, appid, &versions, index, known, &request.path).await?;
        node_ids[index] = outcome.mapped_node.clone();
        outcomes[index] = Some(outcome);
    }
    // Pairs at and below the current manifest: chain the node downward.
    for index in current_index..outcomes.len() {
        let known = node_ids[index].clone().map(|node| (node, Side::Newer));
        let outcome = pair_transition(state, appid, &versions, index, known, &request.path).await?;
        node_ids[index + 1] = outcome.mapped_node.clone();
        outcomes[index] = Some(outcome);
    }

    let mut entries = Vec::with_capacity(versions.len());
    for index in 0..versions.len() {
        let (manifest, _) = &versions[index];
        // The oldest version has no pair below it — it is where the
        // tracked history begins.
        let entry = if let Some(outcome) = outcomes.get(index).and_then(|o| o.as_ref()) {
            FileHistoryEntry {
                manifest: manifest.clone(),
                status: outcome.status,
                previous: Some(versions[index + 1].0.clone()),
                node_id: node_ids[index].clone(),
                diff_node_id: outcome.diff_node_id.clone(),
            }
        } else {
            FileHistoryEntry {
                manifest: manifest.clone(),
                status: HistoryStatus::Initial,
                previous: None,
                node_id: node_ids[index].clone(),
                diff_node_id: None,
            }
        };
        entries.push(entry);
    }
    Ok(entries)
}

fn same_manifest(left: &HistoryManifest, right: &HistoryManifest) -> bool {
    left.depot_id == right.depot_id && left.manifest_id == right.manifest_id
}

/// Which side of a pairwise diff a known node id belongs to.
#[derive(Clone, Copy)]
enum Side {
    /// The newer manifest — the diff's base.
    Newer,
    /// The older manifest — the diff's target.
    Older,
}

/// The transition from `versions[index]` into `versions[index + 1]`.
#[derive(Clone)]
struct PairOutcome {
    status: HistoryStatus,
    /// Row id in the pairwise diff tree, for diff deep links.
    diff_node_id: Option<NodeId>,
    /// Node id on the side opposite `known_node`; `None` ends tracking
    /// there (the file or node doesn't continue).
    mapped_node: Option<NodeId>,
}

/// Compare one consecutive version pair. `known_node` anchors node
/// tracking on one side of the pair; the returned `mapped_node` is the
/// corresponding id on the other side.
async fn pair_transition(
    state: &AppState,
    appid: AppId,
    versions: &[(HistoryManifest, Arc<Snapshot>)],
    index: usize,
    known_node: Option<(NodeId, Side)>,
    path: &str,
) -> Result<PairOutcome> {
    let (newer, newer_snapshot) = &versions[index];
    let (older, older_snapshot) = &versions[index + 1];
    let newer_file = newer_snapshot
        .manifest()
        .files
        .iter()
        .find(|f| f.path == path);
    let older_file = older_snapshot
        .manifest()
        .files
        .iter()
        .find(|f| f.path == path);
    let (status, diff_node_id, mapped_node) = match (newer_file, older_file, known_node) {
        (None, _, _) => (HistoryStatus::Missing, None, None),
        (Some(_), None, _) => (HistoryStatus::Added, None, None),
        (Some(newer_file), Some(older_file), None) if newer_file.sha() == older_file.sha() => {
            (HistoryStatus::Unchanged, None, None)
        }
        (Some(_), Some(_), None) => (HistoryStatus::Changed, None, None),
        (Some(newer_file), Some(older_file), Some((node, _)))
            if newer_file.sha() == older_file.sha() =>
        {
            // Identical content means the same tree — the id carries over.
            (HistoryStatus::Unchanged, None, Some(node))
        }
        (Some(_), Some(_), Some((node, side))) => {
            let diff = build_structured_diff(
                state,
                &StructuredDiffRequest {
                    appid,
                    base: newer.into(),
                    target: older.into(),
                    path: path.to_owned(),
                },
            )
            .await?;
            match diff {
                Some(diff) => match find_diff_node(&diff.root, &node, side) {
                    Some(row) => {
                        let other = match side {
                            Side::Newer => row.diff_match.target.clone(),
                            Side::Older => row.diff_match.base.clone(),
                        };
                        (
                            history_status(row.status.unwrap_or(NodeStatus::Changed)),
                            Some(NodeId(row.id.clone())),
                            other,
                        )
                    }
                    // Diffs omit unchanged subtrees, so no row means the
                    // node is unchanged.
                    None => (HistoryStatus::Unchanged, None, Some(node)),
                },
                None => (HistoryStatus::Changed, None, None),
            }
        }
    };
    Ok(PairOutcome {
        status,
        diff_node_id,
        mapped_node,
    })
}

fn find_diff_node<'a>(
    node: &'a transform::structured::Node,
    id: &NodeId,
    side: Side,
) -> Option<&'a transform::structured::Node> {
    let matched = match side {
        Side::Newer => node.diff_match.base.as_ref() == Some(id),
        Side::Older => node.diff_match.target.as_ref() == Some(id),
    };
    if matched {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_diff_node(child, id, side))
}

fn history_status(status: NodeStatus) -> HistoryStatus {
    match status {
        NodeStatus::Unchanged => HistoryStatus::Unchanged,
        NodeStatus::Changed => HistoryStatus::Changed,
        NodeStatus::Added => HistoryStatus::Added,
        NodeStatus::Removed => HistoryStatus::Removed,
    }
}

/// Input for a depot's version history.
#[derive(Deserialize, ToSchema)]
pub struct ManifestHistoryRequest {
    /// The tracked version set; duplicates by (depot, manifest) keep
    /// their first entry.
    pub manifests: Vec<HistoryManifest>,
}

/// One version of the depot with its transition from the next-older
/// tracked version.
#[derive(Serialize, ToSchema)]
pub struct ManifestHistoryEntry {
    #[serde(flatten)]
    pub manifest: HistoryManifest,
    pub creation_time: u32,
    /// The next-older version; `None` on the oldest row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<HistoryManifest>,
    /// Paths only in this version.
    pub added: u64,
    /// Paths only in the previous version.
    pub removed: u64,
    /// Paths in both versions with different content.
    pub changed: u64,
}

/// Build the depot's version history across the given manifest set,
/// newest first. Like [`build_file_history`], the walk is a property of
/// the version set: there is no "current" version to anchor to, and
/// Steam exposes no complete history API here — the UI hands in the
/// tracked set. Each row's counts come from the manifest metadata only
/// (content sha or symlink target); no file content is downloaded.
pub async fn build_manifest_history(
    state: &AppState,
    appid: AppId,
    request: &ManifestHistoryRequest,
) -> Result<Vec<ManifestHistoryEntry>> {
    let mut versions: Vec<(HistoryManifest, Snapshot)> = Vec::new();
    let mut opens = open_manifests_concurrently(state, appid, &request.manifests, None);
    while let Some((manifest, result)) = opens.next().await {
        versions.push((manifest, result?));
    }
    versions.sort_by_key(|(_, snapshot)| Reverse(snapshot.manifest().creation_time));

    let mut entries = Vec::with_capacity(versions.len());
    for (index, (manifest, snapshot)) in versions.iter().enumerate() {
        let (previous, added, removed, changed) = match versions.get(index + 1) {
            Some((previous, previous_snapshot)) => {
                let (added, removed, changed) =
                    transition_counts(snapshot.manifest(), previous_snapshot.manifest());
                (Some(previous.clone()), added, removed, changed)
            }
            None => (None, 0, 0, 0),
        };
        entries.push(ManifestHistoryEntry {
            manifest: manifest.clone(),
            creation_time: snapshot.manifest().creation_time,
            previous,
            added,
            removed,
            changed,
        });
    }
    Ok(entries)
}

/// Symmetric path-level transition from `older` into `newer`: counts of
/// paths added (only in `newer`), removed (only in `older`), and changed
/// (in both with different content). Directory entries don't count;
/// identity is the content sha or the symlink target.
fn transition_counts(newer: &Manifest, older: &Manifest) -> (u64, u64, u64) {
    let mut older_ids: HashMap<&str, ContentId> = HashMap::with_capacity(older.files.len());
    for file in &older.files {
        if file.is_dir() {
            continue;
        }
        older_ids.insert(file.path.as_str(), content_id(file));
    }

    let mut newer_paths: HashSet<&str> = HashSet::with_capacity(newer.files.len());
    let mut added = 0;
    let mut changed = 0;
    for file in &newer.files {
        if file.is_dir() {
            continue;
        }
        newer_paths.insert(file.path.as_str());
        match older_ids.get(file.path.as_str()) {
            None => added += 1,
            Some(id) if *id != content_id(file) => changed += 1,
            Some(_) => {}
        }
    }
    let removed = older_ids
        .keys()
        .filter(|path| !newer_paths.contains(*path))
        .count() as u64;

    (added, removed, changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use steam_vent_depot::{DepotFile, DepotFileKind, FileHash};

    fn file(path: &str, sha: [u8; 20]) -> DepotFile {
        DepotFile {
            path: path.to_owned(),
            size: 0,
            kind: DepotFileKind::File {
                sha: FileHash(sha),
                executable: false,
                chunks: vec![],
            },
        }
    }

    fn symlink(path: &str, target: &str) -> DepotFile {
        DepotFile {
            path: path.to_owned(),
            size: 0,
            kind: DepotFileKind::Symlink {
                target: target.to_owned(),
            },
        }
    }

    fn manifest(files: Vec<DepotFile>) -> Manifest {
        Manifest {
            depot_id: 1,
            manifest_id: 1,
            creation_time: 0,
            size_uncompressed: 0,
            size_compressed: 0,
            files,
        }
    }

    #[test]
    fn counts_added_removed_changed() {
        let older = manifest(vec![
            file("kept.bin", [1; 20]),
            file("changed.bin", [2; 20]),
            file("removed.bin", [3; 20]),
        ]);
        let newer = manifest(vec![
            file("kept.bin", [1; 20]),
            file("changed.bin", [9; 20]),
            file("added.bin", [4; 20]),
        ]);
        assert_eq!(transition_counts(&newer, &older), (1, 1, 1));
    }

    #[test]
    fn symlink_target_is_identity_and_dirs_dont_count() {
        let older = manifest(vec![
            symlink("link", "old-target"),
            DepotFile {
                path: "dir".to_owned(),
                size: 0,
                kind: DepotFileKind::Directory,
            },
        ]);
        let newer = manifest(vec![
            symlink("link", "new-target"),
            DepotFile {
                path: "dir".to_owned(),
                size: 0,
                kind: DepotFileKind::Directory,
            },
        ]);
        assert_eq!(transition_counts(&newer, &older), (0, 0, 1));
    }
}
