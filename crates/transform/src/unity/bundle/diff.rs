// TODO(ai-review): review for style and correctness
//! Bundle → structured-diff pass. The `build_diff` entry point opens
//! both sides from depot manifests; tests build bundles in memory and
//! call [`build_diff_from_bundles`] directly with their own
//! [`Environment`]/[`BundleFileReader`] pairs.

use std::collections::BTreeMap;
use std::io::Cursor;

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::rabex::files::bundlefile::BundleFileReader;
use rabex_env::rabex::files::unityfile::FileEntry;
use rabex_env::resolver::EnvResolver;
use tracing::info_span;

use crate::structured::{
    Node, NodeStatus, StructuredTree, aggregate_status, human_bytes, prune_unchanged,
};
use crate::unity::relative_to_data_dir;
use crate::unity::serializedfile::diff::diff_sections;
use crate::unity::serializedfile::tree::build_root_node;

use super::{
    ARCHIVE_ID_PREFIX, archive_prefix, blob_node, insert_archive_entry, open_bundle_from_bytes,
};

/// Build the structured diff for a bundle path between two prebuilt
/// envs (`path` depot-absolute, resolved against each side's
/// `data_dir`). Per-entry: SF↔SF runs through [`diff_sections`],
/// blob↔blob is a size compare, mismatched-or-missing entries become
/// fully Added/Removed subtrees. Synchronous; callers from async
/// context must wrap in `tokio::task::spawn_blocking`.
#[tracing::instrument(skip_all, fields(path))]
pub fn build_diff<R: EnvResolver, P: rabex_env::rabex::typetree::TypeTreeProvider>(
    base_env: &Environment<R, P>,
    base_data_dir: &str,
    target_env: &Environment<R, P>,
    target_data_dir: &str,
    path: &str,
) -> Result<StructuredTree> {
    let base_bytes = base_env
        .game_files
        .read_path(std::path::Path::new(relative_to_data_dir(
            base_data_dir,
            path,
        )))?;
    let target_bytes =
        target_env
            .game_files
            .read_path(std::path::Path::new(relative_to_data_dir(
                target_data_dir,
                path,
            )))?;
    let base_bundle = open_bundle_from_bytes(base_env, base_bytes)?;
    let target_bundle = open_bundle_from_bytes(target_env, target_bytes)?;
    build_diff_from_bundles(base_env, &base_bundle, target_env, &target_bundle, path)
}

/// Diff two already-parsed bundles. Same role as
/// [`super::tree::build_tree_from_bundle`] — the prod entrypoint
/// funnels through here after doing the manifest I/O; tests build
/// bundles in memory and call us directly.
pub(crate) fn build_diff_from_bundles<R, P, T>(
    base_env: &Environment<R, P>,
    base_bundle: &BundleFileReader<Cursor<T>>,
    target_env: &Environment<R, P>,
    target_bundle: &BundleFileReader<Cursor<T>>,
    path: &str,
) -> Result<StructuredTree>
where
    R: EnvResolver,
    P: rabex_env::rabex::typetree::TypeTreeProvider,
    T: AsRef<[u8]>,
{
    let base_entries = classify_entries(base_bundle);
    let target_entries = classify_entries(target_bundle);

    // Index target entries by path so we can look up matches in O(1).
    let mut target_by_path: BTreeMap<&str, &BundleEntryView> = BTreeMap::new();
    for e in &target_entries {
        target_by_path.insert(e.path.as_str(), e);
    }

    let mut children: Vec<Node> = Vec::new();
    let _walk = info_span!("walk_entry_pairs", entries = base_entries.len()).entered();
    for be in &base_entries {
        match target_by_path.remove(be.path.as_str()) {
            Some(te) if be.kind == te.kind => match be.kind {
                EntryKind::Serialized => {
                    children.push(diff_archive_pair(
                        base_env,
                        base_bundle,
                        target_env,
                        target_bundle,
                        &be.path,
                    )?);
                }
                EntryKind::Blob => {
                    children.push(diff_blob_pair(&be.path, be.size, te.size));
                }
            },
            Some(te) => {
                // Same path, different kind — vanishingly rare in
                // practice, but model it as remove + add so the user
                // sees both rows.
                children.push(one_sided_entry(
                    target_env,
                    target_bundle,
                    te,
                    NodeStatus::Removed,
                )?);
                children.push(one_sided_entry(
                    base_env,
                    base_bundle,
                    be,
                    NodeStatus::Added,
                )?);
            }
            None => {
                children.push(one_sided_entry(
                    base_env,
                    base_bundle,
                    be,
                    NodeStatus::Added,
                )?);
            }
        }
    }
    // Whatever's left in target_by_path is only-in-target → removed.
    // Preserve the original target order for stable output.
    for te in &target_entries {
        if target_by_path.remove(te.path.as_str()).is_some() {
            children.push(one_sided_entry(
                target_env,
                target_bundle,
                te,
                NodeStatus::Removed,
            )?);
        }
    }

    let status = aggregate_status(&children);
    let pruned = prune_unchanged(children);
    let entry_count = base_entries.len();
    let root = Node {
        id: format!("file:{path}"),
        label: path.to_string(),
        kind: "bundle".to_string(),
        badge: Some(format!(
            "{entry_count} {}",
            if entry_count == 1 { "entry" } else { "entries" }
        )),
        status: Some(status),
        default_collapsed: false,
        children: pruned,
        ..Default::default()
    };

    let mut tree = StructuredTree { root };
    crate::unity::serializedfile::diff::attach_diff_matches(&mut tree.root);
    Ok(tree)
}

/// Sit-classification for one bundle entry — used as the join key
/// between sides. A SerializedFile is matched only against another
/// SerializedFile under the same `path`; same for blobs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Serialized,
    Blob,
}

struct BundleEntryView {
    path: String,
    kind: EntryKind,
    /// Raw on-disk size as reported by the bundle directory. Used for
    /// the blob-diff size comparison and for the badge on blob nodes.
    size: u64,
}

fn classify_entries<T: AsRef<[u8]>>(bundle: &BundleFileReader<Cursor<T>>) -> Vec<BundleEntryView> {
    bundle
        .files()
        .iter()
        .map(|e| BundleEntryView {
            path: e.path.clone(),
            kind: if (e.flags & FileEntry::FLAG_SERIALIZEDFILE) != 0 {
                EntryKind::Serialized
            } else {
                EntryKind::Blob
            },
            size: e.size.max(0) as u64,
        })
        .collect()
}

/// Diff a SerializedFile entry that exists on both sides. Reuses the
/// per-file diff machinery; the resulting subtree gets the same
/// `archive:<entry>/` id prefix as in
/// [`super::tree::build_archive_subtree`] so the node-content endpoint
/// can route lookups back to the right entry.
fn diff_archive_pair<R, P, T>(
    base_env: &Environment<R, P>,
    base_bundle: &BundleFileReader<Cursor<T>>,
    target_env: &Environment<R, P>,
    target_bundle: &BundleFileReader<Cursor<T>>,
    entry_path: &str,
) -> Result<Node>
where
    R: EnvResolver,
    P: rabex_env::rabex::typetree::TypeTreeProvider,
    T: AsRef<[u8]>,
{
    let base_handle = insert_archive_entry(base_env, base_bundle, entry_path)?;
    let target_handle = insert_archive_entry(target_env, target_bundle, entry_path)?;
    let (sections, status) = diff_sections(&base_handle, &target_handle)?;
    let mut node = Node {
        id: format!("{ARCHIVE_ID_PREFIX}{entry_path}"),
        label: entry_path.to_string(),
        kind: "archive".to_string(),
        status: Some(status),
        default_collapsed: false,
        children: sections,
        ..Default::default()
    };
    let prefix = archive_prefix(entry_path);
    for child in &mut node.children {
        child.prefix_ids(&prefix);
    }
    Ok(node)
}

/// Blob entry diff: pure size comparison. Same size → Unchanged
/// (and pruned by the caller); different size → Changed with a
/// "X → Y" badge. We deliberately do not hash the bytes — the
/// user asked for a cheap signal and same-size-different-content
/// blobs are rare for the asset shapes Unity ships.
fn diff_blob_pair(entry_path: &str, base_size: u64, target_size: u64) -> Node {
    let (status, badge) = if base_size == target_size {
        (NodeStatus::Unchanged, human_bytes(base_size))
    } else {
        (
            NodeStatus::Changed,
            format!("{} → {}", human_bytes(target_size), human_bytes(base_size)),
        )
    };
    Node {
        id: format!("blob:{entry_path}"),
        label: entry_path.to_string(),
        kind: "blob".to_string(),
        badge: Some(badge),
        status: Some(status),
        default_collapsed: false,
        ..Default::default()
    }
}

/// Build a fully-Added or fully-Removed subtree for an archive entry
/// that exists only on one side. SerializedFile entries get the same
/// per-file root the non-diff bundle view builds; blob entries are a
/// leaf with the size badge.
fn one_sided_entry<R, P, T>(
    env: &Environment<R, P>,
    bundle: &BundleFileReader<Cursor<T>>,
    entry: &BundleEntryView,
    status: NodeStatus,
) -> Result<Node>
where
    R: EnvResolver,
    P: rabex_env::rabex::typetree::TypeTreeProvider,
    T: AsRef<[u8]>,
{
    let mut node = match entry.kind {
        EntryKind::Serialized => {
            let handle = insert_archive_entry(env, bundle, &entry.path)?;
            let mut subtree = build_root_node(&handle, &entry.path)?;
            subtree.id = format!("{ARCHIVE_ID_PREFIX}{}", entry.path);
            subtree.kind = "archive".to_string();
            let prefix = archive_prefix(&entry.path);
            for child in &mut subtree.children {
                child.prefix_ids(&prefix);
            }
            subtree
        }
        EntryKind::Blob => blob_node(&entry.path, entry.size as i64),
    };
    mark_status_recursive(&mut node, status);
    Ok(node)
}

/// Stamp `status` on `node` and every descendant. Used for one-sided
/// archive entries so every row inside an Added/Removed subtree is
/// tagged consistently (no mixed-status spine).
fn mark_status_recursive(node: &mut Node, status: NodeStatus) {
    node.status = Some(status);
    for child in &mut node.children {
        mark_status_recursive(child, status);
    }
}
