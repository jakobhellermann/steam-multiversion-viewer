// TODO(ai-review): review for style and correctness
//! Tree builder for the addressables catalog: the format is one key →
//! locations map, and the tree mirrors that — key rows hang directly
//! off the file root, grouped by their `/` path segments like the dll
//! namespace tree. Bundle self-reference keys carry their bundle's
//! group and size as the badge.

use std::sync::Arc;

use anyhow::Result;
use rabex_env::addressables::catalog::ResourceLocation;

use crate::structured::{Node, StructuredTree, human_bytes};
use crate::unity::addressables::{CatalogView, bundle_label, parse_catalog, self_bundle, view};

/// Build the catalog's structured tree from the file's bytes.
/// Synchronous; callers from async context must wrap in
/// `tokio::task::spawn_blocking`.
pub fn build_tree(bytes: &[u8], file_label: &str) -> Result<StructuredTree> {
    let catalog = parse_catalog(bytes)?;
    let view = view(&catalog);
    Ok(StructuredTree {
        root: build_root(&view, file_label),
    })
}

pub(crate) fn build_root(view: &CatalogView, file_label: &str) -> Node {
    Node {
        id: format!("file:{file_label}"),
        label: file_label.to_string(),
        kind: "file".to_string(),
        badge: Some(format!("{} keys", view.keys.len())),
        default_collapsed: false,
        children: key_children(view),
        ..Default::default()
    }
}

/// Key rows grouped by their `/` path prefix.
fn key_children(view: &CatalogView) -> Vec<Node> {
    let mut children = Vec::new();
    for (key, locations) in &view.keys {
        let node = key_row(key, locations);
        crate::unity::addressables::insert_key_group(&mut children, key, node);
    }
    set_group_badges(&mut children);
    children
}

/// Label/badge/facet shape shared by single-tree and diff rows.
pub(crate) fn key_row(key: &str, locations: &[Arc<ResourceLocation>]) -> Node {
    let mut node = Node::leaf(format!("key:{key}"), key, "key");
    node.has_content = true;
    if let Some((loc, abro)) = self_bundle(locations) {
        // The key is the bundle's hash-suffixed file name; the badge
        // carries the readable group and the bundle size.
        node.badge = Some(format!(
            "{} · {}",
            bundle_label(loc),
            human_bytes(u64::from(abro.bundle_size))
        ));
    } else if locations.len() == 1 {
        node.badge = Some(locations[0].type_.class_name().to_string());
    } else {
        node.badge = Some(format!("{} locations", locations.len()));
    }
    for loc in locations {
        node.facets
            .entry("provider".to_string())
            .or_insert_with(|| loc.provider_name().to_string());
        node.facets
            .entry("type".to_string())
            .or_insert_with(|| loc.type_.class_name().to_string());
    }
    node
}

/// Leaf count per group for the group badges.
fn set_group_badges(children: &mut [Node]) {
    for child in children.iter_mut().filter(|c| c.kind == "key-group") {
        child.badge = Some(crate::unity::addressables::count_leaves(child).to_string());
        set_group_badges(&mut child.children);
    }
}
