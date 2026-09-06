// TODO(ai-review): review for style and correctness
//! Structured diffs for Unity serialized files.

use std::collections::HashMap;

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

use crate::structured::{Node, NodeStatus, StructuredTree};
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
    let base_relative = path
        .strip_prefix(&format!("{base_data_dir}/"))
        .unwrap_or(path);
    let target_relative = path
        .strip_prefix(&format!("{target_data_dir}/"))
        .unwrap_or(path);
    let base_file = base_env.load_serialized(base_relative)?;
    let target_file = target_env.load_serialized(target_relative)?;

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
    Ok(StructuredTree { root })
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

pub(super) fn pluralize(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

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
