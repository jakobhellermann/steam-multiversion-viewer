// TODO(ai-review): review for style and correctness
//! Diff [`StructuredTree`] builder for .NET assemblies. Calls
//! `dll_diff` to classify each type as added / removed / changed /
//! unchanged, then folds those into the same namespace-grouped tree
//! shape [`super::tree::build_tree`] produces — but with
//! [`NodeStatus`] on every leaf and aggregated upwards onto each
//! namespace.
//!
//! The entity list (`ilspycmd -l`) is fetched for both sides and
//! unioned, so a type present in only one side still appears in the
//! tree with the right `Added` / `Removed` tag.

use std::collections::HashMap;

use camino::Utf8Path;

use super::EntityEntry;
#[cfg(test)]
use super::EntityKind;
use crate::TransformError;
use crate::structured::{Node, NodeStatus, StructuredTree};

/// Build the namespace-grouped diff tree between two .NET assemblies.
/// Runs `dll_diff::diff` on `from_bytes` vs `to_bytes`, fetches the
/// entity listing for both sides, and merges into one tree.
pub async fn build_tree(
    store_root: &Utf8Path,
    from_sha: &[u8; 20],
    from_bytes: &[u8],
    to_sha: &[u8; 20],
    to_bytes: &[u8],
    file_label: &str,
) -> Result<StructuredTree, TransformError> {
    // Both `ilspycmd -l` invocations are cached on dll-sha — repeats
    // are a single file read each, so we can serialise them without
    // pulling in another `try_join`.
    let from_entities = super::list_entities(store_root, from_sha, from_bytes).await?;
    let to_entities = super::list_entities(store_root, to_sha, to_bytes).await?;

    // dll_diff is CPU-heavy (Resolution::parse on both sides). Hand
    // it off to a blocking thread so we don't stall the async
    // executor.
    let from_owned = from_bytes.to_vec();
    let to_owned = to_bytes.to_vec();
    let diff = tokio::task::spawn_blocking(move || dll_diff::diff(&from_owned, &to_owned))
        .await
        .map_err(|e| TransformError::Other(format!("dll-diff task panicked: {e}")))?
        .map_err(|e| TransformError::Other(format!("dll-diff failed: {e}")))?;

    let statuses: HashMap<String, NodeStatus> = diff
        .types
        .into_iter()
        .map(|t| (t.fqn, map_status(t.status)))
        .collect();

    Ok(StructuredTree {
        root: build_root(file_label, &from_entities, &to_entities, &statuses),
    })
}

fn map_status(s: dll_diff::Status) -> NodeStatus {
    match s {
        dll_diff::Status::Added => NodeStatus::Added,
        dll_diff::Status::Removed => NodeStatus::Removed,
        dll_diff::Status::Changed => NodeStatus::Changed,
        dll_diff::Status::Unchanged => NodeStatus::Unchanged,
    }
}

/// Pure helper — drives the existing dll-tree machinery with a
/// status-tagged entity list. Reuses [`super::tree`]'s internal
/// builder by exposing a small adapter: union of entities, then
/// delegate to `tree::build_root` with our augmented `EntityEntry`s.
pub(crate) fn build_root(
    file_label: &str,
    from: &[EntityEntry],
    to: &[EntityEntry],
    statuses: &HashMap<String, NodeStatus>,
) -> Node {
    let union = union_entities(from, to);
    let mut root = super::tree::build_root(file_label, &union);
    // Walk the tree post-order, tagging leaves by FQN lookup and
    // aggregating into parents.
    apply_statuses(&mut root, statuses);
    // Drop Unchanged subtrees so the diff view shows only the spine
    // to every actual change. Same convention as unity::diff.
    prune_unchanged(&mut root);
    root
}

fn union_entities(from: &[EntityEntry], to: &[EntityEntry]) -> Vec<EntityEntry> {
    let mut out: HashMap<String, EntityEntry> = HashMap::new();
    for e in from.iter().chain(to.iter()) {
        out.entry(e.name.clone()).or_insert_with(|| e.clone());
    }
    let mut v: Vec<EntityEntry> = out.into_values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Walk the tree, set `status` on each leaf via the FQN map, and
/// aggregate to parents:
///   * any descendant changed → parent changed
///   * else all descendants unchanged → parent unchanged
///
/// Added / Removed don't propagate upward — a namespace stays
/// `changed` when it gains/loses entries.
fn apply_statuses(node: &mut Node, statuses: &HashMap<String, NodeStatus>) {
    if node.id.starts_with("type:") {
        let status = statuses
            .get(node.id.strip_prefix("type:").unwrap())
            .copied()
            .unwrap_or(NodeStatus::Unchanged);
        node.status = Some(status);
        // Tag one-sided rows with the side prefix so the content
        // endpoint dumps only the manifest the type actually lives on.
        // Matched rows (Changed / Unchanged) keep the bare `type:` id
        // — both sides hold the same FQN and the endpoint dumps both.
        match status {
            NodeStatus::Added => node.id = format!("base:{}", node.id),
            NodeStatus::Removed => node.id = format!("target:{}", node.id),
            NodeStatus::Changed | NodeStatus::Unchanged => {}
        }
    }
    let mut child_status = ChildStatus::default();
    for child in &mut node.children {
        apply_statuses(child, statuses);
        if let Some(s) = child.status {
            child_status.observe(s);
        }
    }
    if node.id.starts_with("ns:") || node.id.starts_with("file:") {
        node.status = Some(child_status.aggregate());
    }
}

/// Post-order drop: any node whose own status is `Unchanged` AND
/// whose entire subtree is `Unchanged` goes away. The root is left
/// in place even if it ends up empty so the frontend always has a
/// `file:`-id container to mount on.
fn prune_unchanged(node: &mut Node) {
    for child in &mut node.children {
        prune_unchanged(child);
    }
    node.children
        .retain(|c| !matches!(c.status, Some(NodeStatus::Unchanged)));
}

#[derive(Default)]
struct ChildStatus {
    any_changed: bool,
    seen_any: bool,
}

impl ChildStatus {
    fn observe(&mut self, s: NodeStatus) {
        self.seen_any = true;
        if !matches!(s, NodeStatus::Unchanged) {
            self.any_changed = true;
        }
    }
    fn aggregate(self) -> NodeStatus {
        if !self.seen_any {
            NodeStatus::Unchanged
        } else if self.any_changed {
            NodeStatus::Changed
        } else {
            NodeStatus::Unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str) -> EntityEntry {
        EntityEntry {
            kind: EntityKind::Class,
            name: name.to_string(),
        }
    }

    #[test]
    fn leaf_tags_propagate_and_unchanged_pruned() {
        let from = vec![class("UnityEngine.UI.Image"), class("UnityEngine.UI.Text")];
        let to = from.clone();
        let mut statuses = HashMap::new();
        statuses.insert("UnityEngine.UI.Image".to_string(), NodeStatus::Changed);
        statuses.insert("UnityEngine.UI.Text".to_string(), NodeStatus::Unchanged);
        let root = build_root("x", &from, &to, &statuses);
        assert_eq!(root.status, Some(NodeStatus::Changed));
        let ue = &root.children[0];
        assert_eq!(ue.label, "UnityEngine");
        assert_eq!(ue.status, Some(NodeStatus::Changed));
        let ui = &ue.children[0];
        assert_eq!(ui.label, "UI");
        assert_eq!(ui.status, Some(NodeStatus::Changed));
        // Only the changed leaf survives the prune; the unchanged
        // `Text` sibling is dropped to keep the diff view to the
        // actual spine of changes.
        assert_eq!(ui.children.len(), 1);
        assert_eq!(ui.children[0].label, "Image");
        assert_eq!(ui.children[0].status, Some(NodeStatus::Changed));
    }

    #[test]
    fn entirely_unchanged_subtree_collapses() {
        let from = vec![
            class("Demo.Touched"),
            class("Demo.Untouched"),
            class("Other.Stable.A"),
            class("Other.Stable.B"),
        ];
        let to = from.clone();
        let mut statuses = HashMap::new();
        statuses.insert("Demo.Touched".into(), NodeStatus::Changed);
        statuses.insert("Demo.Untouched".into(), NodeStatus::Unchanged);
        statuses.insert("Other.Stable.A".into(), NodeStatus::Unchanged);
        statuses.insert("Other.Stable.B".into(), NodeStatus::Unchanged);
        let root = build_root("x", &from, &to, &statuses);
        // `Other.Stable.*` are all unchanged → the whole `Other`
        // namespace disappears. Only `Demo.Touched` survives.
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].label, "Demo");
        assert_eq!(root.children[0].children.len(), 1);
        assert_eq!(root.children[0].children[0].label, "Touched");
    }

    #[test]
    fn one_side_only_appears_in_union() {
        let from = vec![class("Demo.Old")];
        let to = vec![class("Demo.New")];
        let mut statuses = HashMap::new();
        statuses.insert("Demo.Old".to_string(), NodeStatus::Removed);
        statuses.insert("Demo.New".to_string(), NodeStatus::Added);
        let root = build_root("x", &from, &to, &statuses);
        let demo = &root.children[0];
        assert_eq!(demo.label, "Demo");
        assert_eq!(demo.children.len(), 2);
        // Aggregated namespace shows changed because of Added/Removed.
        assert_eq!(demo.status, Some(NodeStatus::Changed));
    }
}
