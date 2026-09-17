//! Structured tree builders for Unity serialized files.

mod components;
mod hierarchy;
mod shader;

use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

use crate::structured::{Node, StructuredTree};

pub(super) use super::{go_label, pluralize};

/// Resolves an object node id to its path id.
pub fn parse_object_node_id(id: &str) -> Option<PathId> {
    id.strip_prefix("obj:").and_then(|n| n.parse().ok())
}

/// Builds the structured tree for one serialized file.
pub fn build_tree<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    data_dir: &str,
    path: &str,
) -> Result<StructuredTree> {
    let relative = path.strip_prefix(&format!("{data_dir}/")).unwrap_or(path);
    let file = env.load_serialized(relative)?;
    let root = build_root_node(&file, path)?;
    Ok(StructuredTree { root })
}

/// Builds the root node for one serialized file.
#[tracing::instrument(skip_all, fields(label))]
pub fn build_root_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    label: &str,
) -> Result<Node> {
    let stats = collect_class_stats(file);
    let hierarchy = hierarchy::build_section(file)?;
    let loose = build_loose_section(file, &hierarchy.covered)?;
    let class_stats = build_class_stats_section(&stats);
    let total_objects: usize = stats.values().sum();

    Ok(Node {
        id: format!("file:{label}"),
        label: label.to_string(),
        kind: "file".to_string(),
        badge: Some(format!(
            "{total_objects} {}",
            pluralize(total_objects, "object")
        )),
        default_collapsed: false,
        children: vec![class_stats, hierarchy.node, loose],
        ..Default::default()
    })
}

fn collect_class_stats<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
) -> BTreeMap<ClassId, usize> {
    let mut counts = BTreeMap::new();
    for obj in file.file.objects() {
        *counts.entry(obj.m_ClassID).or_default() += 1;
    }
    counts
}

fn build_class_stats_section(counts: &BTreeMap<ClassId, usize>) -> Node {
    let children = counts
        .iter()
        .map(|(class_id, count)| {
            Node::leaf(
                format!("class:{class_id:?}"),
                format!("{class_id:?}"),
                "class-stat",
            )
            .with_badge(format!("×{count}"))
        })
        .collect();
    let total: usize = counts.values().sum();
    Node {
        id: "section:class-stats".to_string(),
        label: "Class stats".to_string(),
        kind: "section".to_string(),
        badge: Some(format!("{total} {}", pluralize(total, "object"))),
        default_collapsed: true,
        children,
        ..Default::default()
    }
}

fn build_loose_section<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    covered: &HashSet<PathId>,
) -> Result<Node> {
    let mut children = Vec::new();
    for obj in file.file.objects() {
        if covered.contains(&obj.m_PathID) {
            continue;
        }
        children.push(components::component_node(
            file,
            obj.m_PathID,
            obj.m_ClassID,
            true,
        )?);
    }
    let count = children.len();
    Ok(Node {
        id: "section:loose".to_string(),
        label: "Loose components".to_string(),
        kind: "section".to_string(),
        badge: Some(count.to_string()),
        default_collapsed: false,
        children,
        ..Default::default()
    })
}
