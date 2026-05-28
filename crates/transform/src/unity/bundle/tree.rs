// TODO(ai-review): review for style and correctness
//! Bundle → structured-tree pass. The `build_tree` entry point does
//! the manifest I/O; tests build bundles in memory and call
//! [`build_tree_from_bundle`] directly with their own
//! [`Environment`]/[`BundleFileReader`].

use std::io::Cursor;

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use tracing::info_span;

use crate::structured::{Node, StructuredTree};
use crate::unity::serializedfile::tree::{TREE_KIND, build_root_node};

use super::{
    ARCHIVE_ID_PREFIX, BUNDLE_ENTRY_FLAG_SERIALIZED_FILE, archive_prefix, blob_node,
    insert_archive_entry,
};

/// Construct the structured tree for the bundle at `path` (manifest-
/// relative) using a prebuilt `env`. `bundle_bytes` must be supplied
/// by the caller (typically `env.game_files.read_path(...)`) so the
/// I/O stays at the route layer. Synchronous; callers from async
/// context must wrap in `tokio::task::spawn_blocking`.
#[tracing::instrument(skip_all, fields(path))]
pub fn build_tree<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    bundle_bytes: rabex_env::env::Data,
    path: &str,
) -> Result<StructuredTree> {
    // Bundles don't carry a unity-version header — the version lives in
    // the SerializedFiles inside, which the reader hasn't parsed yet at
    // open time. Pull the version from globalgamemanagers via the env
    // and hand it over as the bundle's fallback.
    let unity_version = env.unity_version()?.clone();
    let bundle = {
        let _span = info_span!("parse_bundle_header").entered();
        let config = ExtractionConfig::default().with_fallback_unity_version(unity_version);
        BundleFileReader::from_reader(Cursor::new(bundle_bytes.as_ref()), &config)?
    };

    build_tree_from_bundle(env, &bundle, path)
}

/// Walk an already-parsed bundle and produce its structured tree.
/// Lives here so tests can build bundles in memory (no manifest store)
/// and exercise the same orchestration the prod entrypoint uses.
pub(crate) fn build_tree_from_bundle<R, P, T>(
    env: &Environment<R, P>,
    bundle: &BundleFileReader<Cursor<T>>,
    path: &str,
) -> Result<StructuredTree>
where
    R: EnvResolver,
    P: rabex_env::rabex::typetree::TypeTreeProvider,
    T: AsRef<[u8]>,
{
    let mut children = Vec::new();
    let _walk = info_span!("walk_entries", entries = bundle.files().len()).entered();
    for entry in bundle.files() {
        let is_serialized = (entry.flags & BUNDLE_ENTRY_FLAG_SERIALIZED_FILE) != 0;
        children.push(if is_serialized {
            build_archive_subtree(env, bundle, &entry.path)?
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

/// Build the subtree for one SerializedFile-flagged bundle entry. The
/// root id is replaced with the `archive:<entry>` form and every
/// descendant id gets the `archive:<entry>/` prefix so they stay
/// unique across sibling archive entries (each SerializedFile has its
/// own `obj:42`).
#[tracing::instrument(skip_all, fields(entry_path))]
pub(super) fn build_archive_subtree<R, P, T>(
    env: &Environment<R, P>,
    bundle: &BundleFileReader<Cursor<T>>,
    entry_path: &str,
) -> Result<Node>
where
    R: EnvResolver,
    P: rabex_env::rabex::typetree::TypeTreeProvider,
    T: AsRef<[u8]>,
{
    let handle = insert_archive_entry(env, bundle, entry_path)?;
    let mut subtree = build_root_node(&handle, entry_path)?;
    subtree.id = format!("{ARCHIVE_ID_PREFIX}{entry_path}");
    subtree.kind = "archive".to_string();
    let prefix = archive_prefix(entry_path);
    for child in &mut subtree.children {
        child.prefix_ids(&prefix);
    }
    Ok(subtree)
}
