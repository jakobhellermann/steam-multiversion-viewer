// TODO(ai-review): review for style and correctness
//! Structured diff for the addressables catalog: regular keys match by
//! their string; bundle self-reference keys follow their bundle's
//! stable `bundle_name`, so a rebuild shows as one `changed` row,
//! `mod:`-paired across the differing file names.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use rabex_env::addressables::catalog::ResourceLocation;

use crate::structured::{
    Node, NodeStatus, StructuredTree, aggregate_status, human_bytes, prune_unchanged,
};
use crate::unity::addressables::{
    CatalogView, bundle_label, location_signature, parse_catalog, self_bundle, view,
};
use crate::unity::serializedfile::diff::attach_diff_matches;

/// Build the catalog diff from both sides' bytes. Synchronous; callers
/// from async context must wrap in `tokio::task::spawn_blocking`.
pub fn build_diff(
    base_bytes: &[u8],
    target_bytes: &[u8],
    file_label: &str,
) -> Result<StructuredTree> {
    let base = view(&parse_catalog(base_bytes)?);
    let target = view(&parse_catalog(target_bytes)?);
    Ok(build_diff_from_views(&base, &target, file_label))
}

pub(crate) fn build_diff_from_views(
    base: &CatalogView,
    target: &CatalogView,
    file_label: &str,
) -> StructuredTree {
    let rows = diff_key_rows(base, target);
    let mut children = Vec::new();
    for row in rows {
        let label = row.label.clone();
        crate::unity::addressables::insert_key_group(&mut children, &label, row);
    }
    set_group_statuses(&mut children);
    prune_groups(&mut children);
    let status = aggregate_status(&children);
    let mut root = Node {
        id: format!("file:{file_label}"),
        label: file_label.to_string(),
        kind: "file".to_string(),
        badge: Some(format!("{}→{} keys", base.keys.len(), target.keys.len())),
        status: Some(status),
        children,
        ..Default::default()
    };
    attach_diff_matches(&mut root);
    StructuredTree { root }
}

fn diff_key_rows(base: &CatalogView, target: &CatalogView) -> Vec<Node> {
    let target_self_keys = self_keys(target);
    let base_self_keys = self_keys(base);

    let mut rows = Vec::new();
    for (key, locations) in &base.keys {
        match self_bundle(locations) {
            Some((_, abro)) => {
                rows.push(match target_self_keys.get(abro.bundle_name.as_str()) {
                    Some((target_key, target_locations)) => {
                        matched_self_key_row(key, locations, target_key, target_locations)
                    }
                    None => one_sided_key_row(key, locations, NodeStatus::Added, Side::Base),
                });
            }
            None => {
                rows.push(match target.keys.get(key) {
                    Some(target_locations) => matched_key_row(key, locations, target_locations),
                    None => one_sided_key_row(key, locations, NodeStatus::Added, Side::Base),
                });
            }
        }
    }
    for (key, locations) in &target.keys {
        // Self-keys of bundles that exist on both sides were already
        // paired in the base loop; only genuinely new bundles remain.
        let handled = self_bundle(locations)
            .is_some_and(|(_, abro)| base_self_keys.contains_key(abro.bundle_name.as_str()));
        if !handled && !base.keys.contains_key(key) {
            rows.push(one_sided_key_row(
                key,
                locations,
                NodeStatus::Removed,
                Side::Target,
            ));
        }
    }
    rows
}

/// Bundle self-reference keys of one side, indexed by stable name.
fn self_keys(view: &CatalogView) -> BTreeMap<String, (&String, &[Arc<ResourceLocation>])> {
    view.keys
        .iter()
        .filter_map(|(key, locations)| {
            self_bundle(locations)
                .map(|(_, abro)| (abro.bundle_name.to_string(), (key, locations.as_slice())))
        })
        .collect()
}

/// Bundle self-key on both sides: `changed` when the file name (content
/// hash) moved; `mod:`-paired when the names differ. The badge carries
/// the size delta, or `rebuilt` when only the hash moved.
fn matched_self_key_row(
    key: &str,
    base: &[Arc<ResourceLocation>],
    target_key: &str,
    target: &[Arc<ResourceLocation>],
) -> Node {
    let mut node = crate::unity::addressables::tree::key_row(key, base);
    if let (Some((base_loc, base_abro)), Some((_, target_abro))) =
        (self_bundle(base), self_bundle(target))
    {
        let group = bundle_label(base_loc);
        node.badge = Some(if base_abro.bundle_size != target_abro.bundle_size {
            format!(
                "{group} · {} → {}",
                human_bytes(u64::from(base_abro.bundle_size)),
                human_bytes(u64::from(target_abro.bundle_size))
            )
        } else {
            format!("{group} · rebuilt")
        });
    }
    if key == target_key {
        node.id = format!("key:{key}");
    } else {
        node.id = format!("mod:key:{key},key:{target_key}");
    }
    node.status = Some(changed_status(key != target_key));
    node
}

/// Regular key on both sides: `changed` when a location's provider,
/// internal id, type, or dependency bundles moved.
fn matched_key_row(
    key: &str,
    base: &[Arc<ResourceLocation>],
    target: &[Arc<ResourceLocation>],
) -> Node {
    let changed = signatures(base) != signatures(target);
    let mut node = crate::unity::addressables::tree::key_row(key, base);
    node.status = Some(changed_status(changed));
    node
}

fn signatures(locations: &[Arc<ResourceLocation>]) -> Vec<String> {
    let mut sigs: Vec<String> = locations.iter().map(|l| location_signature(l)).collect();
    sigs.sort();
    sigs
}

fn one_sided_key_row(
    key: &str,
    locations: &[Arc<ResourceLocation>],
    status: NodeStatus,
    side: Side,
) -> Node {
    let mut node = crate::unity::addressables::tree::key_row(key, locations);
    node.id = format!("{}key:{key}", side.prefix());
    node.status = Some(status);
    node
}

fn changed_status(changed: bool) -> NodeStatus {
    if changed {
        NodeStatus::Changed
    } else {
        NodeStatus::Unchanged
    }
}

/// Side marker for one-sided diff node ids.
enum Side {
    Base,
    Target,
}

impl Side {
    fn prefix(&self) -> &'static str {
        match self {
            Side::Base => "base:",
            Side::Target => "target:",
        }
    }
}

/// Group statuses from their (still unpruned) children and leaf counts
/// for the badges.
fn set_group_statuses(children: &mut [Node]) {
    for child in children.iter_mut() {
        set_group_statuses(&mut child.children);
        if child.kind == "key-group" {
            child.status = Some(aggregate_status(&child.children));
            child.badge = Some(crate::unity::addressables::count_leaves(child).to_string());
        }
    }
}

/// Drop unchanged leaves bottom-up; groups whose subtree emptied go too.
fn prune_groups(children: &mut Vec<Node>) {
    for child in children.iter_mut().filter(|c| c.kind == "key-group") {
        prune_groups(&mut child.children);
        child.children = prune_unchanged(std::mem::take(&mut child.children));
    }
    *children = prune_unchanged(std::mem::take(children));
}
