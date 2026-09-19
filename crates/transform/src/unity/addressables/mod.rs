// TODO(ai-review): review for style and correctness
//! Addressables content catalog (`StreamingAssets/aa/catalog.bin`): the
//! map from keys (addresses, labels, GUIDs, bundle file names) to
//! resource locations, plus one `AssetBundleProvider` location per
//! bundle. Tree/diff builders work off the parsed catalog; per-node
//! content is dumped as JSON by [`dump_key_json`].

mod diff;
mod tree;

pub use diff::build_diff;
pub use tree::build_tree;

#[cfg(test)]
mod test;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use rabex_env::addressables::catalog::{
    AddressablesCatalog, AssetBundleRequestOptions, ResourceLocation, resource_providers,
};

use crate::structured::Node;

/// Nest a key leaf under its `/` path prefix, creating `key-group`
/// nodes on the way (`Scenes/Peak_07` nests under `Scenes`; keys
/// without `/` land at the top level).
pub(crate) fn insert_key_group(children: &mut Vec<Node>, key: &str, leaf: Node) {
    let segments: Vec<&str> = key.split('/').collect();
    let mut children = children;
    for i in 0..segments.len().saturating_sub(1) {
        let prefix = segments[..=i].join("/");
        let existing = children
            .iter()
            .position(|c| c.kind == "key-group" && c.label == prefix);
        children = match existing {
            Some(idx) => &mut children[idx].children,
            None => {
                children.push(Node {
                    id: format!("key-group:{prefix}"),
                    label: prefix.to_string(),
                    kind: "key-group".to_string(),
                    badge: None,
                    default_collapsed: true,
                    children: Vec::new(),
                    ..Default::default()
                });
                let group = children.last_mut().expect("group just pushed");
                &mut group.children
            }
        };
    }
    children.push(leaf);
}

/// Leaf count per `key-group` subtree, for badges.
pub(crate) fn count_leaves(node: &Node) -> usize {
    if node.kind != "key-group" {
        return 1;
    }
    node.children.iter().map(count_leaves).sum()
}

/// `key:<…>` node id → the catalog key.
pub fn parse_key_node_id(id: &str) -> Option<&str> {
    id.strip_prefix("key:")
}

pub(crate) fn parse_catalog(bytes: &[u8]) -> Result<AddressablesCatalog> {
    AddressablesCatalog::from_reader(Cursor::new(bytes)).context("parsing addressables catalog")
}

/// Keys with their locations, sorted. Keys whose locations are all
/// `AssetBundleProvider` are the bundles' own self-references (their
/// key is the hash-suffixed file name).
pub(crate) struct CatalogView {
    pub keys: BTreeMap<String, Vec<Arc<ResourceLocation>>>,
}

pub(crate) fn view(catalog: &AddressablesCatalog) -> CatalogView {
    let keys = catalog
        .resources
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    CatalogView { keys }
}

/// A location's `AssetBundleRequestOptions`, when the location is a bundle.
pub(crate) fn bundle_of(loc: &ResourceLocation) -> Option<&AssetBundleRequestOptions> {
    if loc.provider_id.as_ref() == resource_providers::ASSET_BUNDLE {
        loc.data.as_ref()
    } else {
        None
    }
}

/// The bundle containing a (non-bundle) location: its first
/// `AssetBundleProvider` dependency.
pub(crate) fn containing_bundle(loc: &ResourceLocation) -> Option<&ResourceLocation> {
    loc.dependencies
        .iter()
        .find(|dep| dep.provider_id.as_ref() == resource_providers::ASSET_BUNDLE)
        .map(|dep| dep.as_ref())
}

/// The location and bundle options behind a bundle self-reference key.
pub(crate) fn self_bundle(
    locations: &[Arc<ResourceLocation>],
) -> Option<(&ResourceLocation, &AssetBundleRequestOptions)> {
    locations
        .iter()
        .find_map(|loc| bundle_of(loc).map(|abro| (loc.as_ref(), abro)))
}

/// Signature of what a key resolves to: provider, internal id, type,
/// and the dependency bundles' stable names — file-name hash churn
/// stays outside it.
pub(crate) fn location_signature(loc: &ResourceLocation) -> String {
    let deps = loc
        .dependencies
        .iter()
        .map(|dep| match bundle_of(dep) {
            Some(abro) => abro.bundle_name.to_string(),
            None => dep.internal_id.to_string(),
        })
        .collect::<Vec<_>>();
    format!(
        "{}|{}|{}|{}",
        loc.provider_name(),
        loc.internal_id,
        loc.type_.class_name(),
        deps.join(",")
    )
}

const RUNTIME_PATH: &str = "{UnityEngine.AddressableAssets.Addressables.RuntimePath}";

/// Bundle label: the hash-stripped file name from the location's
/// primary key.
pub(crate) fn bundle_label(loc: &ResourceLocation) -> String {
    strip_hash_suffix(&loc.primary_key)
}

/// internal_id → data-dir-relative file path: replaces the RuntimePath
/// placeholder with `StreamingAssets/aa` and normalizes `\` to `/`.
pub(crate) fn runtime_file_path(internal_id: &str) -> String {
    internal_id
        .replace(RUNTIME_PATH, "StreamingAssets/aa")
        .replace('\\', "/")
}

/// File name with the `_<hash32>` build suffix before `.bundle` stripped.
pub(crate) fn strip_hash_suffix(file_name: &str) -> String {
    let Some(stem) = file_name.strip_suffix(".bundle") else {
        return file_name.to_owned();
    };
    match stem.rsplit_once('_') {
        Some((group, hash)) if is_hash32(hash) => group.to_owned(),
        _ => file_name.to_owned(),
    }
}

fn is_hash32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// JSON body for `key:<key>` nodes: the key's locations with their
/// resolved dependency bundles. For a bundle's self-reference key, the
/// reverse index joins: the keys whose locations resolve into this
/// bundle.
pub fn dump_key_json(bytes: &[u8], key: &str) -> Result<String> {
    let catalog = parse_catalog(bytes)?;
    let locations = catalog
        .resources
        .iter()
        .find(|(k, _)| k.as_str() == key)
        .map(|(_, v)| v)
        .with_context(|| format!("key not in catalog: {key}"))?;
    let mut body = serde_json::json!({
        "key": key,
        "locations": locations.iter().map(|l| location_json(l)).collect::<Vec<_>>(),
    });
    if let Some((_, abro)) = self_bundle(locations) {
        body["resident_keys"] = serde_json::json!(bundle_residents(&catalog, &abro.bundle_name));
    }
    Ok(serde_json::to_string_pretty(&body)?)
}

/// Keys whose locations resolve directly into the bundle `bundle_name`.
fn bundle_residents(catalog: &AddressablesCatalog, bundle_name: &str) -> Vec<String> {
    catalog
        .resources
        .iter()
        .filter(|(_, locations)| {
            locations.iter().any(|loc| {
                containing_bundle(loc).is_some_and(|b| {
                    bundle_of(b).is_some_and(|abro| abro.bundle_name.as_str() == bundle_name)
                })
            })
        })
        .map(|(key, _)| key.to_string())
        .collect()
}

fn location_json(loc: &ResourceLocation) -> serde_json::Value {
    serde_json::json!({
        "provider": loc.provider_name(),
        "type": loc.type_.class_name(),
        "internal_id": runtime_file_path(&loc.internal_id),
        "dependencies": loc
            .dependencies
            .iter()
            .map(|d| dependency_label(d))
            .collect::<Vec<_>>(),
        "bundle": loc.data.as_ref().map(|abro| serde_json::json!({
            "name": abro.bundle_name.as_str(),
            "size": abro.bundle_size,
            "crc": abro.crc,
        })),
    })
}

fn dependency_label(dep: &ResourceLocation) -> String {
    if bundle_of(dep).is_some() {
        bundle_label(dep)
    } else {
        dep.internal_id.to_string()
    }
}
