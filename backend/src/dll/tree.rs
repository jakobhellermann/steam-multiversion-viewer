// TODO(ai-review): review for style and correctness
//! Build a [`StructuredTree`] for a .NET assembly. The hierarchy
//! mirrors namespaces:
//!
//! ```text
//! Assembly-CSharp.dll      (badge: total entity count)
//! ├─ (global)              (badge: type count without a namespace)
//! │  └─ <Module>
//! ├─ UnityEngine           (badge: subtree size)
//! │  ├─ UI
//! │  │  ├─ Image
//! │  │  └─ ...
//! │  └─ Vector3
//! └─ TMProOld
//!    └─ ...
//! ```
//!
//! Leaves carry the entity kind (class / interface / struct /
//! delegate / enum) as `kind`. Their `id` is `type:<fully-qualified>`
//! so the lazy `/structured/node` endpoint can hand it back to
//! [`crate::dll::decompile_type`].

use std::collections::BTreeMap;

use camino::Utf8Path;

use super::EntityEntry;
#[cfg(test)]
use super::EntityKind;
use crate::structured::{Node, StructuredTree};
use crate::transform::TransformError;

/// Tree-`kind` identifier used in [`StructuredTree::kind`]. The
/// frontend keys off this to pick renderer behaviour.
pub const TREE_KIND: &str = "dll-types";

/// Build the namespace-grouped tree for `dll_bytes`. Calls
/// `ilspycmd -l` under the hood (cached by sha).
pub async fn build_tree(
    store_root: &Utf8Path,
    dll_sha: &[u8; 20],
    dll_bytes: &[u8],
    file_label: &str,
) -> Result<StructuredTree, TransformError> {
    let entities = super::list_entities(store_root, dll_sha, dll_bytes).await?;
    Ok(StructuredTree {
        kind: TREE_KIND.to_string(),
        root: build_root(file_label, &entities),
    })
}

/// Pure helper — assemble the tree out of a list of entities. Split
/// from `build_tree` so it's unit-testable without needing ilspy on
/// PATH.
fn build_root(file_label: &str, entities: &[EntityEntry]) -> Node {
    let mut root = NsBuilder::new("");
    for entity in entities {
        root.insert(entity);
    }
    let children = root.into_children();
    Node {
        id: format!("file:{file_label}"),
        label: file_label.to_string(),
        kind: "file".to_string(),
        badge: Some(format!("{} entities", entities.len())),
        default_collapsed: false,
        children,
    }
}

/// In-progress representation of a namespace. Holds child namespaces
/// keyed by their next segment plus the leaf entities directly in
/// this namespace.
struct NsBuilder {
    /// Fully-qualified namespace path, e.g. `UnityEngine.UI`. Empty
    /// at the root and at the synthetic `(global)` bucket.
    path: String,
    children: BTreeMap<String, NsBuilder>,
    leaves: Vec<EntityEntry>,
}

impl NsBuilder {
    fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            children: BTreeMap::new(),
            leaves: Vec::new(),
        }
    }

    fn insert(&mut self, entity: &EntityEntry) {
        let (namespace, _) = split_namespace(&entity.name);
        let mut cur = self;
        for segment in namespace.split('.').filter(|s| !s.is_empty()) {
            cur = cur.children.entry(segment.to_string()).or_insert_with(|| {
                let path = if cur.path.is_empty() {
                    segment.to_string()
                } else {
                    format!("{}.{}", cur.path, segment)
                };
                NsBuilder::new(path)
            });
        }
        cur.leaves.push(entity.clone());
    }

    /// Total entity count contained under this builder, recursively.
    fn entity_count(&self) -> usize {
        self.leaves.len()
            + self
                .children
                .values()
                .map(|c| c.entity_count())
                .sum::<usize>()
    }

    fn into_children(self) -> Vec<Node> {
        // Render order: leaves directly here ("globals" of this
        // namespace) before sub-namespaces. Both alphabetical via
        // BTreeMap / pre-sorted vec.
        let mut leaves = self.leaves;
        leaves.sort_by(|a, b| a.name.cmp(&b.name));
        let mut globals: Vec<Node> = Vec::new();
        for entity in leaves {
            globals.push(leaf_node(&entity));
        }
        let mut sub_ns: Vec<Node> = Vec::new();
        for (segment, child) in self.children {
            sub_ns.push(child.into_namespace_node(segment));
        }
        // Globals first, then namespaces.
        globals.extend(sub_ns);
        globals
    }

    fn into_namespace_node(self, segment: String) -> Node {
        let count = self.entity_count();
        Node {
            id: format!("ns:{}", self.path),
            label: segment,
            kind: "namespace".to_string(),
            badge: Some(format!("{count}")),
            default_collapsed: false,
            children: self.into_children(),
        }
    }
}

fn leaf_node(entity: &EntityEntry) -> Node {
    // Leaf labels show just the last segment — the parent namespace
    // chain is already implied by indentation in the tree.
    let (_, leaf) = split_namespace(&entity.name);
    Node {
        id: format!("type:{}", entity.name),
        label: leaf.to_string(),
        kind: entity.kind.as_str().to_string(),
        badge: None,
        default_collapsed: false,
        children: Vec::new(),
    }
}

/// Split a fully-qualified type name into `(namespace, leaf)`. For
/// `UnityEngine.UI.Image` that's `("UnityEngine.UI", "Image")`; for
/// `<Module>` that's `("", "<Module>")`.
fn split_namespace(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) => (&name[..dot], &name[dot + 1..]),
        None => ("", name),
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
    fn groups_by_namespace() {
        let entities = vec![
            class("UnityEngine.UI.Image"),
            class("UnityEngine.UI.Text"),
            class("UnityEngine.Vector3"),
            class("<Module>"),
        ];
        let root = build_root("Assembly-CSharp.dll", &entities);
        assert_eq!(root.children.len(), 2);
        // Leaves of the root (no namespace) first.
        assert_eq!(root.children[0].label, "<Module>");
        assert_eq!(root.children[0].kind, "class");
        // Namespace nodes after.
        assert_eq!(root.children[1].label, "UnityEngine");
        assert_eq!(root.children[1].badge.as_deref(), Some("3"));
        let ue = &root.children[1];
        assert_eq!(ue.children.len(), 2);
        // Vector3 leaf before UI namespace (leaves first).
        assert_eq!(ue.children[0].label, "Vector3");
        assert_eq!(ue.children[1].label, "UI");
        let ui = &ue.children[1];
        assert_eq!(ui.children.len(), 2);
        assert_eq!(ui.children[0].label, "Image");
        assert_eq!(ui.children[1].label, "Text");
    }

    #[test]
    fn entity_kind_propagated() {
        let entities = vec![EntityEntry {
            kind: EntityKind::Enum,
            name: "Foo.Bar".to_string(),
        }];
        let root = build_root("x", &entities);
        let ns = &root.children[0];
        assert_eq!(ns.children[0].kind, "enum");
    }
}
