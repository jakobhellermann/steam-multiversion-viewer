use std::collections::BTreeMap;

use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

use crate::structured::{Node, NodeStatus};

#[tracing::instrument(skip_all)]
pub(super) fn diff_class_stats<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    target_file: &SerializedFileHandle<'_, R, P>,
) -> Node {
    let (base_counts, base_total) = count_classes(base_file);
    let (target_counts, target_total) = count_classes(target_file);
    let mut all_classes = BTreeMap::new();
    for class_id in base_counts.keys().chain(target_counts.keys()) {
        all_classes.insert(*class_id, ());
    }

    let children: Vec<Node> = all_classes
        .keys()
        .filter_map(|class_id| {
            let (status, badge) = match (
                base_counts.get(class_id).copied(),
                target_counts.get(class_id).copied(),
            ) {
                (Some(base), None) => (NodeStatus::Added, format!("×{base}")),
                (None, Some(target)) => (NodeStatus::Removed, format!("×{target}")),
                (Some(base), Some(target)) if base == target => return None,
                (Some(base), Some(target)) => (NodeStatus::Changed, format!("×{target} → ×{base}")),
                (None, None) => unreachable!(),
            };
            Some(Node {
                badge: Some(badge),
                ..super::make_node(
                    format!("class:{class_id:?}"),
                    format!("{class_id:?}"),
                    "class-stat",
                    status,
                )
            })
        })
        .collect();
    let status = if children.is_empty() && base_total == target_total {
        NodeStatus::Unchanged
    } else {
        NodeStatus::Changed
    };
    Node {
        badge: Some(total_badge(base_total, target_total)),
        children,
        default_collapsed: true,
        ..super::make_node("section:class-stats", "Class stats", "section", status)
    }
}

fn count_classes<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
) -> (BTreeMap<ClassId, usize>, usize) {
    let mut counts = BTreeMap::new();
    for object in file.file.objects() {
        *counts.entry(object.m_ClassID).or_default() += 1;
    }
    let total = counts.values().sum();
    (counts, total)
}

fn total_badge(base: usize, target: usize) -> String {
    if base == target {
        format!("{base} {}", super::pluralize(base, "object"))
    } else {
        format!(
            "{target} {} → {base} {}",
            super::pluralize(target, "object"),
            super::pluralize(base, "object")
        )
    }
}
