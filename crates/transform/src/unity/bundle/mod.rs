// TODO(ai-review): review for style and correctness
//! Structured tree + diff for Unity bundle / `.unity3d` files.
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
//! Per-archive subtrees reuse
//! [`crate::unity::serializedfile::tree::build_root_node`] and get
//! their ids namespaced via [`crate::structured::Node::prefix_ids`] —
//! the `archive:<entry>/` prefix is what
//! [`crate::unity::serializedfile::dump_value::dump_bundle_object_json`]
//! later parses back out of the per-node content id.
//!
//! The two entry points (`build_tree*` / `build_diff*`) live in the
//! `tree` and `diff` submodules; their public symbols are re-exported
//! here so call sites still write `unity::bundle::build_tree` etc.

use std::io::Cursor;

use anyhow::{Context, Result};
use rabex_env::Environment;
use rabex_env::env::Data;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::files::SerializedFile;
use rabex_env::rabex::files::bundlefile::BundleFileReader;
use rabex_env_steam_depot_vfs::SteamDepotGameFiles;
use steam_depot_vfs::chunk_store::ChunkStore;

use crate::structured::Node;

pub mod diff;
pub mod tree;

#[cfg(test)]
mod test;

pub use diff::build_diff;
pub use tree::build_tree;

/// Prefix used for ids inside one archive entry. Followed by the bare
/// id produced by the per-file builders (`obj:N`, `section:…`).
pub(super) const ARCHIVE_ID_PREFIX: &str = "archive:";

/// Bit on `BundleEntry::flags` that marks an entry as a SerializedFile
/// (vs. a raw blob like `*.resource`). Matches what
/// `BundleFileReader::serialized_files` filters on internally; we
/// inline the check here so we only walk the entry list once.
pub(crate) const BUNDLE_ENTRY_FLAG_SERIALIZED_FILE: u32 = 4;

/// Build the wire-form `archive:<entry>/` prefix for namespacing a
/// per-file subtree's ids.
pub(super) fn archive_prefix(entry: &str) -> String {
    format!("{ARCHIVE_ID_PREFIX}{entry}/")
}

/// Split an `archive:<entry>/<inner>` id back into its components, or
/// return `None` for ids that don't carry the bundle prefix.
pub fn parse_archive_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix(ARCHIVE_ID_PREFIX)?;
    let slash = rest.find('/')?;
    Some((&rest[..slash], &rest[slash + 1..]))
}

pub(super) fn blob_node(entry_path: &str, size: i64) -> Node {
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

pub(super) fn human_bytes(n: u64) -> String {
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
pub(super) fn strip_data_prefix<'a, C: ChunkStore>(
    game_files: &SteamDepotGameFiles<C>,
    path: &'a str,
) -> &'a str {
    let data_dir = game_files.data_dir().display().to_string();
    path.strip_prefix(&format!("{data_dir}/")).unwrap_or(path)
}

/// Load a SerializedFile entry's bytes from the bundle and stash it in
/// the side's env cache so PPtr resolution inside `diff_sections` can
/// find it. Returns a handle keyed under the bare archive entry path.
pub(super) fn insert_archive_entry<'env, R, P, T>(
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
