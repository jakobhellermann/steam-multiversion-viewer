// TODO(ai-review): review for style and correctness
//! Structured diffs for Unity serialized files.

use std::collections::HashMap;

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

use crate::structured::{Node, NodeId, NodeStatus, StructuredTree};
use crate::unity::relative_to_data_dir;
pub(super) use equality::{matched_status, object_bytes};

mod class_stats;
mod equality;
mod hierarchy;
mod loose;

/// Object bytes indexed by path id.
pub(super) type BodyIndex<'a> = HashMap<PathId, &'a [u8]>;

#[tracing::instrument(skip_all)]
pub(super) fn build_body_index<'a, R: EnvResolver, P>(
    file: &SerializedFileHandle<'a, R, P>,
) -> BodyIndex<'a> {
    file.file
        .objects()
        .map(|o| (o.m_PathID, object_bytes(file, o)))
        .collect()
}

/// Builds a node with diff defaults.
pub(super) fn make_node(
    id: impl Into<String>,
    label: impl Into<String>,
    kind: impl Into<String>,
    status: NodeStatus,
) -> Node {
    let kind = kind.into();
    let has_content = matches!(kind.as_str(), "gameobject" | "component");
    let default_collapsed = kind == "gameobject";
    Node {
        id: id.into(),
        label: label.into(),
        kind,
        status: Some(status),
        has_content,
        default_collapsed,
        ..Default::default()
    }
}

/// Side of a one-sided diff node.
#[derive(Clone, Copy, Debug)]
pub(super) enum Side {
    Base,
    Target,
}

impl Side {
    fn prefix(self) -> &'static str {
        match self {
            Side::Base => "base",
            Side::Target => "target",
        }
    }
}

/// Builds an id for a matched object pair.
pub(super) fn matched_pair_id(base: PathId, target: PathId) -> String {
    if base == target {
        format!("obj:{base}")
    } else {
        format!("mod:obj:{base},obj:{target}")
    }
}

/// Builds an id for a one-sided object.
pub(super) fn one_sided_id(side: Side, path_id: PathId) -> String {
    format!("{}:obj:{path_id}", side.prefix())
}

/// Builds the structured diff for one file.
#[tracing::instrument(skip_all, fields(path))]
pub fn build_diff<R: EnvResolver, P: TypeTreeProvider>(
    base_env: &Environment<R, P>,
    base_data_dir: &str,
    target_env: &Environment<R, P>,
    target_data_dir: &str,
    path: &str,
) -> Result<StructuredTree> {
    let base_file = base_env.load_serialized(relative_to_data_dir(base_data_dir, path))?;
    let target_file = target_env.load_serialized(relative_to_data_dir(target_data_dir, path))?;

    let (children, status) = diff_sections(&base_file, &target_file)?;
    let root = Node {
        id: format!("file:{path}"),
        label: path.to_string(),
        kind: "file".to_string(),
        badge: None,
        status: Some(status),
        children,
        ..Default::default()
    };
    let mut tree = StructuredTree { root };
    attach_diff_matches(&mut tree.root);
    Ok(tree)
}

/// Record the version-local object ids represented by every diff row.
pub(crate) fn attach_diff_matches(node: &mut Node) {
    let (prefix, id) = match crate::unity::bundle::parse_archive_id(&node.id) {
        Some((archive, id)) => (format!("archive:{archive}/"), id),
        None => (String::new(), node.id.as_str()),
    };
    node.diff_match = match id {
        id if let Some(id) = id.strip_prefix("base:") => crate::structured::DiffNodeMatch {
            base: Some(NodeId(format!("{prefix}{id}"))),
            target: None,
        },
        id if let Some(id) = id.strip_prefix("target:") => crate::structured::DiffNodeMatch {
            base: None,
            target: Some(NodeId(format!("{prefix}{id}"))),
        },
        id if let Some(ids) = id.strip_prefix("mod:") => {
            if let Some((base, target)) = ids.split_once(',') {
                crate::structured::DiffNodeMatch {
                    base: Some(NodeId(format!("{prefix}{base}"))),
                    target: Some(NodeId(format!("{prefix}{target}"))),
                }
            } else {
                crate::structured::DiffNodeMatch::default()
            }
        }
        id => crate::structured::DiffNodeMatch {
            base: Some(NodeId(format!("{prefix}{id}"))),
            target: Some(NodeId(format!("{prefix}{id}"))),
        },
    };
    for child in &mut node.children {
        attach_diff_matches(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_current_and_previous_object_ids() {
        let mut root = Node::leaf("file:x", "x", "file");
        root.children = vec![
            Node::leaf("obj:1", "same", "component"),
            Node::leaf("mod:obj:2,obj:9", "moved", "component"),
            Node::leaf("base:obj:3", "added", "component"),
            Node::leaf("target:obj:4", "removed", "component"),
        ];

        attach_diff_matches(&mut root);

        assert_eq!(
            root.children[0].diff_match.base,
            Some(NodeId("obj:1".into()))
        );
        assert_eq!(
            root.children[0].diff_match.target,
            Some(NodeId("obj:1".into()))
        );
        assert_eq!(
            root.children[1].diff_match.base,
            Some(NodeId("obj:2".into()))
        );
        assert_eq!(
            root.children[1].diff_match.target,
            Some(NodeId("obj:9".into()))
        );
        assert_eq!(
            root.children[2].diff_match.base,
            Some(NodeId("obj:3".into()))
        );
        assert_eq!(root.children[2].diff_match.target, None);
        assert_eq!(root.children[3].diff_match.base, None);
        assert_eq!(
            root.children[3].diff_match.target,
            Some(NodeId("obj:4".into()))
        );
    }
}

/// Builds the class, hierarchy, and loose-object sections.
pub(crate) fn diff_sections<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    target_file: &SerializedFileHandle<'_, R, P>,
) -> Result<(Vec<Node>, NodeStatus)> {
    let class_stats = class_stats::diff_class_stats(base_file, target_file);
    let (hierarchy, covered) = hierarchy::diff_hierarchy(base_file, target_file)?;
    let loose = loose::diff_loose(base_file, target_file, &covered)?;

    let children = prune_unchanged(vec![class_stats, hierarchy, loose]);
    let status = aggregate_status(&children);
    Ok((children, status))
}

// ---- shared helpers ------------------------------------------------------

pub(super) use super::{go_label, pluralize};

pub(super) fn prune_unchanged(mut children: Vec<Node>) -> Vec<Node> {
    children.retain(|c| c.status != Some(NodeStatus::Unchanged) || !c.children.is_empty());
    children
}

pub(super) fn aggregate_status(children: &[Node]) -> NodeStatus {
    if children
        .iter()
        .any(|c| c.status != Some(NodeStatus::Unchanged))
    {
        NodeStatus::Changed
    } else {
        NodeStatus::Unchanged
    }
}
