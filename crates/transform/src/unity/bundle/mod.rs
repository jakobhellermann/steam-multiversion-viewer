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
        badge: Some(crate::structured::human_bytes(size.max(0) as u64)),
        default_collapsed: false,
        children: Vec::new(),
        ..Default::default()
    }
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
    let mut sf = SerializedFile::from_reader(&mut Cursor::new(bytes.as_slice()))?;
    // Bundle entry SerializedFiles omit the unity version (it lives at
    // the bundle level); backfill from the env so file-version-dependent
    // reads (e.g. GameObject::path) resolve instead of erroring.
    if sf.m_UnityVersion.is_none() {
        sf.m_UnityVersion = Some(env.unity_version()?.clone());
    }
    Ok(env.insert_cache(entry_path.into(), sf, Data::InMemory(bytes)))
}
