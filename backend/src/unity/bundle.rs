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

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rabex_env::Environment;
use rabex_env::env::Data;
use rabex_env::rabex::files::SerializedFile;
use rabex_env::rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex_env::resolver::EnvResolver;
use rabex_env_steam_depot_vfs::SteamDepotGameFiles;
use steam_depot_vfs::chunk_store::ChunkStore;
use steam_depot_vfs::fs::DepotManifestStore;
use tracing::info_span;

use crate::structured::{Node, StructuredTree};

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
