// TODO(ai-review): review for style and correctness
//! Structured tree for Unity bundle / `.unity3d` files.
//!
//! A bundle is a container of one or more files. The usual layout is
//! one "main" SerializedFile plus zero or more sibling SerializedFiles
//! (`*.sharedAssets`) and raw blobs (`*.resource`, `*.resS`). We
//! mirror that on the tree:
//!
//! ```text
//! <bundle path>
//! ├─ CAB-xxx           (SerializedFile — full class-stats/hierarchy/loose subtree)
//! ├─ CAB-xxx.sharedAssets
//! └─ CAB-xxx.resource  (raw blob, badge: <size>)
//! ```
//!
//! Per-archive subtrees reuse [`super::tree::build_root_node`] and get
//! their ids namespaced via [`crate::structured::Node::prefix_ids`] —
//! the `archive:<entry>/` prefix is what
//! [`super::dump_value::dump_bundle_object_json`] later parses back out
//! of the per-node content id.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rabex_env::Environment;
use rabex_env::env::Data;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::files::SerializedFile;
use rabex_env::rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex_env::resolver::EnvResolver;
use rabex_env_steam_depot_vfs::SteamDepotGameFiles;
use steam_depot_vfs::chunk_store::ChunkStore;
use steam_depot_vfs::fs::DepotManifestStore;
use tracing::info_span;

use crate::structured::{Node, NodeStatus, StructuredTree};

use super::tree::{TREE_KIND, build_root_node};

/// Prefix used for ids inside one archive entry. Followed by the bare
/// id produced by the per-file builders (`obj:N`, `section:…`).
const ARCHIVE_ID_PREFIX: &str = "archive:";

/// Bit on `BundleEntry::flags` that marks an entry as a SerializedFile
/// (vs. a raw blob like `*.resource`). Matches what
/// `BundleFileReader::serialized_files` filters on internally; we
/// inline the check here so we only walk the entry list once.
const BUNDLE_ENTRY_FLAG_SERIALIZED_FILE: u32 = 4;

/// Build the wire-form `archive:<entry>/` prefix for namespacing a
/// per-file subtree's ids.
fn archive_prefix(entry: &str) -> String {
    format!("{ARCHIVE_ID_PREFIX}{entry}/")
}

/// Split an `archive:<entry>/<inner>` id back into its components, or
/// return `None` for ids that don't carry the bundle prefix.
pub fn parse_archive_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix(ARCHIVE_ID_PREFIX)?;
    let slash = rest.find('/')?;
    Some((&rest[..slash], &rest[slash + 1..]))
}

/// Construct the structured tree for the bundle at `path`. Synchronous;
/// callers from async context must wrap in `tokio::task::spawn_blocking`.
#[tracing::instrument(skip_all, fields(path))]
pub fn build_tree<C: ChunkStore + 'static>(
    manifest_store: Arc<DepotManifestStore<C>>,
    path: &str,
) -> Result<StructuredTree> {
    let game_files = SteamDepotGameFiles::new(manifest_store)?;
    let relative = strip_data_prefix(&game_files, path).to_owned();

    let raw = {
        let _span = info_span!("read_bundle_bytes").entered();
        game_files
            .read_path(Path::new(&relative))
            .with_context(|| format!("reading bundle bytes {relative}"))?
    };

    let tpk = info_span!("get tpk").in_scope(|| TypeTreeCache::new(TpkTypeTreeBlob::embedded()));

    let env = Environment::new(game_files, &tpk);
    // Bundles don't carry a unity-version header — the version lives in
    // the SerializedFiles inside, which the reader hasn't parsed yet at
    // open time. Pull the version from globalgamemanagers via the env
    // and hand it over as the bundle's fallback.
    let unity_version = env.unity_version()?.clone();
    let bundle = {
        let _span = info_span!("parse_bundle_header").entered();
        let config = ExtractionConfig::default().with_fallback_unity_version(unity_version);
        BundleFileReader::from_reader(Cursor::new(raw.as_ref()), &config)?
    };

    let mut children = Vec::new();
    let _walk = info_span!("walk_entries", entries = bundle.files().len()).entered();
    for entry in bundle.files() {
        let is_serialized = (entry.flags & BUNDLE_ENTRY_FLAG_SERIALIZED_FILE) != 0;
        children.push(if is_serialized {
            build_archive_subtree(&env, &bundle, &entry.path)?
        } else {
            blob_node(&entry.path, entry.size)
        });
    }

    let entry_count = children.len();
    let root = Node {
        id: format!("file:{path}"),
        label: path.to_string(),
        kind: "bundle".to_string(),
        badge: Some(format!(
            "{entry_count} {}",
            if entry_count == 1 { "entry" } else { "entries" }
        )),
        default_collapsed: false,
        children,
        ..Default::default()
    };

    Ok(StructuredTree {
        kind: TREE_KIND.to_string(),
        root,
    })
}

#[tracing::instrument(skip_all, fields(entry_path))]
fn build_archive_subtree<'env, R, P, T>(
    env: &'env Environment<R, P>,
    bundle: &BundleFileReader<Cursor<T>>,
    entry_path: &str,
) -> Result<Node>
where
    R: rabex_env::resolver::EnvResolver,
    P: rabex_env::rabex::typetree::TypeTreeProvider,
    T: AsRef<[u8]>,
{
    let bytes = bundle
        .read_at(entry_path)?
        .with_context(|| format!("entry {entry_path} unexpectedly absent"))?;
    let sf = SerializedFile::from_reader(&mut Cursor::new(bytes.as_slice()))?;
    let handle = env.insert_cache(entry_path.into(), sf, Data::InMemory(bytes));
    let mut subtree = build_root_node(&handle, entry_path)?;
    // Replace the root's id (which `build_root_node` set to
    // `file:<entry>`) with our archive header form, then namespace all
    // descendant ids — they collide otherwise across sibling archive
    // entries (every SerializedFile has its own `obj:42`).
    subtree.id = format!("{ARCHIVE_ID_PREFIX}{entry_path}");
    subtree.kind = "archive".to_string();
    let prefix = archive_prefix(entry_path);
    for child in &mut subtree.children {
        child.prefix_ids(&prefix);
    }
    Ok(subtree)
}

fn blob_node(entry_path: &str, size: i64) -> Node {
    Node {
        id: format!("blob:{entry_path}"),
        label: entry_path.to_string(),
        kind: "blob".to_string(),
        badge: Some(human_bytes(size.max(0) as u64)),
        default_collapsed: false,
        children: Vec::new(),
        ..Default::default()
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Strip the leading `<DataDir>/` (e.g. `silksong_Data/`) from a
/// manifest-relative path so the result is what rabex-env's resolvers
/// expect.
fn strip_data_prefix<'a, C: ChunkStore>(
    game_files: &SteamDepotGameFiles<C>,
    path: &'a str,
) -> &'a str {
    let data_dir = game_files.data_dir().display().to_string();
    path.strip_prefix(&format!("{data_dir}/")).unwrap_or(path)
}

/// One side opened for bundle diffing. Owns the env + bundle reader so
/// the handles handed to [`build_diff`] live as long as the side does.
struct OpenedBundle<C: ChunkStore + 'static> {
    env: Environment<SteamDepotGameFiles<C>, TypeTreeCache<TpkTypeTreeBlob>>,
    bundle: BundleFileReader<Cursor<Data>>,
}

#[tracing::instrument(skip_all, fields(path))]
fn open_bundle<C: ChunkStore + 'static>(
    manifest_store: Arc<DepotManifestStore<C>>,
    path: &str,
) -> Result<OpenedBundle<C>> {
    let game_files = SteamDepotGameFiles::new(manifest_store)?;
    let relative = strip_data_prefix(&game_files, path).to_owned();
    let raw = {
        let _span = info_span!("read_bundle_bytes").entered();
        game_files
            .read_path(Path::new(&relative))
            .with_context(|| format!("reading bundle bytes {relative}"))?
    };
    let tpk = info_span!("get tpk").in_scope(|| TypeTreeCache::new(TpkTypeTreeBlob::embedded()));
    let env = Environment::new(game_files, tpk);
    let unity_version = {
        let _span = info_span!("unity_version").entered();
        env.unity_version()?.clone()
    };
    let bundle = {
        let _span = info_span!("parse_bundle_header").entered();
        let config = ExtractionConfig::default().with_fallback_unity_version(unity_version);
        BundleFileReader::from_reader(Cursor::new(raw), &config)?
    };
    Ok(OpenedBundle { env, bundle })
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
            kind: if (e.flags & BUNDLE_ENTRY_FLAG_SERIALIZED_FILE) != 0 {
                EntryKind::Serialized
            } else {
                EntryKind::Blob
            },
            size: e.size.max(0) as u64,
        })
        .collect()
}

/// Load a SerializedFile entry's bytes from the bundle and stash it in
/// the side's env cache so PPtr resolution inside `diff_sections` can
/// find it. Returns a handle keyed under the bare archive entry path.
fn insert_archive_entry<'env, R, P, T>(
    env: &'env Environment<R, P>,
    bundle: &BundleFileReader<Cursor<T>>,
    entry_path: &str,
) -> Result<SerializedFileHandle<'env, R, P>>
where
    R: rabex_env::resolver::EnvResolver,
    P: rabex_env::rabex::typetree::TypeTreeProvider,
    T: AsRef<[u8]>,
{
    let bytes = bundle
        .read_at(entry_path)?
        .with_context(|| format!("entry {entry_path} unexpectedly absent"))?;
    let sf = SerializedFile::from_reader(&mut Cursor::new(bytes.as_slice()))?;
    Ok(env.insert_cache(entry_path.into(), sf, Data::InMemory(bytes)))
}

/// Build the structured diff for a bundle path between the two
/// manifests. Per-entry: SF↔SF runs through [`super::diff::diff_sections`],
/// blob↔blob is a size compare, mismatched-or-missing entries become
/// fully Added/Removed subtrees. Synchronous; callers from async
/// context must wrap in `tokio::task::spawn_blocking`.
#[tracing::instrument(skip_all, fields(path))]
pub fn build_diff<C: ChunkStore + 'static>(
    base_manifest: Arc<DepotManifestStore<C>>,
    target_manifest: Arc<DepotManifestStore<C>>,
    path: &str,
) -> Result<StructuredTree> {
    let base = open_bundle(base_manifest, path).context("base side")?;
    let target = open_bundle(target_manifest, path).context("target side")?;

    let base_entries = classify_entries(&base.bundle);
    let target_entries = classify_entries(&target.bundle);

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
                    children.push(diff_archive_pair(&base, &target, &be.path)?);
                }
                EntryKind::Blob => {
                    children.push(diff_blob_pair(&be.path, be.size, te.size));
                }
            },
            Some(te) => {
                // Same path, different kind — vanishingly rare in
                // practice, but model it as remove + add so the user
                // sees both rows.
                children.push(one_sided_entry(&target, te, NodeStatus::Removed)?);
                children.push(one_sided_entry(&base, be, NodeStatus::Added)?);
            }
            None => {
                children.push(one_sided_entry(&base, be, NodeStatus::Added)?);
            }
        }
    }
    // Whatever's left in target_by_path is only-in-target → removed.
    // Preserve the original target order for stable output.
    for te in &target_entries {
        if target_by_path.remove(te.path.as_str()).is_some() {
            children.push(one_sided_entry(&target, te, NodeStatus::Removed)?);
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

    Ok(StructuredTree {
        kind: TREE_KIND.to_string(),
        root,
    })
}

/// Diff a SerializedFile entry that exists on both sides. Reuses the
/// per-file diff machinery; the resulting subtree gets the same
/// `archive:<entry>/` id prefix as in [`build_tree`] so the node-content
/// endpoint can route lookups back to the right entry.
fn diff_archive_pair<C: ChunkStore + 'static>(
    base: &OpenedBundle<C>,
    target: &OpenedBundle<C>,
    entry_path: &str,
) -> Result<Node> {
    let base_handle = insert_archive_entry(&base.env, &base.bundle, entry_path)?;
    let target_handle = insert_archive_entry(&target.env, &target.bundle, entry_path)?;
    let (sections, status) = super::diff::diff_sections(&base_handle, &target_handle)?;
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
fn one_sided_entry<C: ChunkStore + 'static>(
    side: &OpenedBundle<C>,
    entry: &BundleEntryView,
    status: NodeStatus,
) -> Result<Node> {
    let mut node = match entry.kind {
        EntryKind::Serialized => {
            let handle = insert_archive_entry(&side.env, &side.bundle, &entry.path)?;
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

/// Drop entries that came back as Unchanged with no informative
/// children. Mirrors `unity::diff::prune_unchanged` so the bundle root
/// has the same "spine to change only" shape as the per-file diff.
fn prune_unchanged(mut children: Vec<Node>) -> Vec<Node> {
    children.retain(|c| c.status != Some(NodeStatus::Unchanged) || !c.children.is_empty());
    children
}

fn aggregate_status(children: &[Node]) -> NodeStatus {
    if children
        .iter()
        .any(|c| c.status != Some(NodeStatus::Unchanged) && c.status.is_some())
    {
        NodeStatus::Changed
    } else {
        NodeStatus::Unchanged
    }
}
