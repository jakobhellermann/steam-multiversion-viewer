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

use std::collections::{BTreeMap, HashSet};

use camino::Utf8Path;

use super::EntityEntry;
#[cfg(test)]
use super::EntityKind;
use crate::TransformError;
use crate::structured::{Node, StructuredTree};

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
        root: build_root(file_label, &entities),
    })
}

/// Pure helper — assemble the tree out of a list of entities. Split
/// from `build_tree` so it's unit-testable without needing ilspy on
/// PATH, and reused by [`super::diff`] for the diff-tagged variant.
pub fn build_root(file_label: &str, entities: &[EntityEntry]) -> Node {
    // ilspy's `-l` output prints nested types as `Outer.Inner`, the same
    // shape as a namespaced type — there's no syntactic marker. But the
    // listing also contains the outer type as its own entry. So a `.`
    // is a nested separator iff the prefix-up-to-the-dot is itself an
    // entity in the listing. Build a name-set up front for O(1)
    // longest-prefix lookups during insertion.
    let entity_names: HashSet<&str> = entities.iter().map(|e| e.name.as_str()).collect();

    // Sort entities by name length so any outer type is inserted before
    // its nested children — keeps the `nested_under` lookup simple and
    // avoids out-of-order placement.
    let mut sorted: Vec<&EntityEntry> = entities.iter().collect();
    sorted.sort_by_key(|e| (e.name.len(), e.name.as_str()));

    let mut root = NsBuilder::new("");
    for entity in sorted {
        root.insert(entity, &entity_names);
    }
    // Bucket types without a namespace into a synthetic "-" node so the
    // root listing isn't drowned in 800+ flat leaves. Skip when empty.
    let mut leaves: Vec<LeafBuilder> = root.leaves.into_values().collect();
    leaves.sort_by(|a, b| a.entity.name.cmp(&b.entity.name));
    let mut children: Vec<Node> = Vec::new();
    if !leaves.is_empty() {
        let count = leaves.len();
        let global_children: Vec<Node> = leaves.into_iter().map(LeafBuilder::into_node).collect();
        children.push(Node {
            id: "ns:-".to_string(),
            label: "-".to_string(),
            kind: "namespace".to_string(),
            badge: Some(format!("{count}")),
            // .NET assemblies are huge — start with namespaces folded
            // so the root listing is browsable instead of dumping
            // thousands of types up-front.
            default_collapsed: true,
            children: global_children,
            ..Default::default()
        });
    }
    for (segment, child) in root.children {
        children.push(child.into_namespace_node(segment));
    }
    Node {
        id: format!("file:{file_label}"),
        label: file_label.to_string(),
        kind: "file".to_string(),
        badge: Some(format!("{} entities", entities.len())),
        default_collapsed: false,
        children,
        ..Default::default()
    }
}

/// In-progress representation of a namespace. Holds child namespaces
/// keyed by their next segment plus the leaf entities directly in
/// this namespace, keyed by their fully-qualified name so nested
/// types can be appended onto an outer type that landed here earlier.
struct NsBuilder {
    /// Fully-qualified namespace path, e.g. `UnityEngine.UI`. Empty
    /// at the root and at the synthetic "-" bucket.
    path: String,
    children: BTreeMap<String, NsBuilder>,
    leaves: BTreeMap<String, LeafBuilder>,
}

/// In-progress representation of an entity that may itself host
/// nested types (also entities).
struct LeafBuilder {
    entity: EntityEntry,
    nested: BTreeMap<String, LeafBuilder>,
}

impl LeafBuilder {
    fn new(entity: EntityEntry) -> Self {
        Self {
            entity,
            nested: BTreeMap::new(),
        }
    }

    fn into_node(self) -> Node {
        let mut node = leaf_node(&self.entity);
        // Nested types (and anything below them, transitively) only
        // matter when the user is searching — flag the whole subtree
        // so the frontend hides it by default but surfaces it on a
        // label match.
        node.children = self
            .nested
            .into_values()
            .map(|c| {
                let mut sub = c.into_node();
                mark_hidden(&mut sub);
                sub
            })
            .collect();
        node
    }
}

fn mark_hidden(node: &mut Node) {
    node.hide_unless_matched = true;
    for c in &mut node.children {
        mark_hidden(c);
    }
}

impl LeafBuilder {
    fn count(&self) -> usize {
        1 + self.nested.values().map(LeafBuilder::count).sum::<usize>()
    }
}

impl NsBuilder {
    fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            children: BTreeMap::new(),
            leaves: BTreeMap::new(),
        }
    }

    fn insert(&mut self, entity: &EntityEntry, entity_names: &HashSet<&str>) {
        // Walk the dotted name left-to-right. While the prefix is a
        // known namespace (no entity with that prefix exists), descend
        // into namespace sub-builders. The first time we hit a prefix
        // that *is* an entity in the listing, switch to nested-type
        // mode and walk leaves instead.
        let (ns_prefix, type_chain) = split_at_first_entity_prefix(&entity.name, entity_names);
        let mut cur_ns = self;
        for segment in ns_prefix.split('.').filter(|s| !s.is_empty()) {
            let parent_path = cur_ns.path.clone();
            cur_ns = cur_ns
                .children
                .entry(segment.to_string())
                .or_insert_with(|| {
                    let path = if parent_path.is_empty() {
                        segment.to_string()
                    } else {
                        format!("{parent_path}.{segment}")
                    };
                    NsBuilder::new(path)
                });
        }
        // type_chain is the dotted run of entity names from the outer
        // type down to `entity` itself. Walk it the same way but through
        // LeafBuilder::nested rather than NsBuilder::children. Each
        // intermediate name must already exist as an entity (asserted
        // by `split_at_first_entity_prefix`).
        let mut chain = type_chain.split('.').filter(|s| !s.is_empty()).peekable();
        let mut cursor_leaves = &mut cur_ns.leaves;
        let mut acc_name = ns_prefix.to_string();
        while let Some(segment) = chain.next() {
            if !acc_name.is_empty() {
                acc_name.push('.');
            }
            acc_name.push_str(segment);
            let is_last = chain.peek().is_none();
            let entry = cursor_leaves
                .entry(acc_name.clone())
                .or_insert_with(|| LeafBuilder::new(placeholder_entity(&acc_name)));
            if is_last {
                // Real data for `entity` — overwrite the placeholder
                // (or keep what's there if we got here via the outer
                // already being seeded).
                entry.entity = entity.clone();
            }
            cursor_leaves = &mut entry.nested;
        }
    }

    /// Total entity count contained under this builder, recursively.
    fn entity_count(&self) -> usize {
        self.leaves.values().map(LeafBuilder::count).sum::<usize>()
            + self
                .children
                .values()
                .map(|c| c.entity_count())
                .sum::<usize>()
    }

    fn into_children(self) -> Vec<Node> {
        // Render order: leaves directly here ("globals" of this
        // namespace) before sub-namespaces. Both alphabetical via
        // the BTreeMap iteration.
        let mut globals: Vec<Node> = self
            .leaves
            .into_values()
            .map(LeafBuilder::into_node)
            .collect();
        for (segment, child) in self.children {
            globals.push(child.into_namespace_node(segment));
        }
        globals
    }

    fn into_namespace_node(self, segment: String) -> Node {
        let count = self.entity_count();
        Node {
            id: format!("ns:{}", self.path),
            label: segment,
            kind: "namespace".to_string(),
            badge: Some(format!("{count}")),
            // Match the synthetic "-" bucket — keep all namespaces
            // folded until the user opens them.
            default_collapsed: true,
            children: self.into_children(),
            ..Default::default()
        }
    }
}

/// Split a dotted name into `(namespace_prefix, type_chain)`. The
/// type_chain starts at the first segment whose prefix is itself an
/// entity in the listing — every segment after that is a nested type
/// under the previous one.
///
/// `Foo.Bar.Baz` where `Foo.Bar` is an entity → `("Foo", "Bar.Baz")`.
/// `Foo.Bar.Baz` where nothing matches → `("Foo.Bar", "Baz")` (the
/// final segment is always the leaf, dotted prefix is the namespace).
fn split_at_first_entity_prefix<'a>(
    name: &'a str,
    entity_names: &HashSet<&str>,
) -> (&'a str, &'a str) {
    // Iterate dot positions; for each one, ask "is the part up to here
    // a known entity?". First hit splits the name into "namespace of
    // that outer type" + "outer-and-everything-after".
    for (idx, _) in name.match_indices('.') {
        let prefix = &name[..idx];
        if entity_names.contains(prefix) {
            let outer_ns = name_namespace(prefix);
            let chain_start = if outer_ns.is_empty() {
                0
            } else {
                outer_ns.len() + 1
            };
            return (outer_ns, &name[chain_start..]);
        }
    }
    // No entity prefix matched → classic namespace.leaf split.
    match name.rfind('.') {
        Some(i) => (&name[..i], &name[i + 1..]),
        None => ("", name),
    }
}

fn name_namespace(name: &str) -> &str {
    name.rfind('.').map(|i| &name[..i]).unwrap_or("")
}

/// Stand-in entity used when a nested type is inserted before its
/// outer type. The real entity overwrites it once seen. In practice
/// the build_root sorts by length, so outer always lands first and
/// these never actually surface — but keeping the fallback makes
/// `insert` robust against unsorted input.
fn placeholder_entity(name: &str) -> EntityEntry {
    EntityEntry {
        kind: super::EntityKind::Class,
        name: name.to_string(),
    }
}

fn leaf_node(entity: &EntityEntry) -> Node {
    // Leaf labels show just the last segment — the parent namespace
    // chain is already implied by indentation in the tree.
    let (_, leaf) = split_namespace(&entity.name);
    let kind = entity.kind.as_str();
    Node {
        id: format!("type:{}", entity.name),
        label: leaf.to_string(),
        kind: kind.to_string(),
        // Same string in `kind` and in the `kind` facet, but the facet
        // is what the frontend filter UI uses; `kind` is just the
        // renderer hint. Cheap to duplicate.
        facets: [("kind".to_string(), kind.to_string())]
            .into_iter()
            .collect(),
        has_content: true,
        ..Default::default()
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
        // Unnamespaced types live under a synthetic "-" bucket (matches
        // Avalonia ILSpy's UI convention for "no namespace").
        assert_eq!(root.children[0].label, "-");
        assert_eq!(root.children[0].kind, "namespace");
        assert_eq!(root.children[0].badge.as_deref(), Some("1"));
        assert_eq!(root.children[0].children[0].label, "<Module>");
        assert_eq!(root.children[0].children[0].kind, "class");
        // Real namespace nodes follow.
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

    #[test]
    fn nested_types_attach_to_outer() {
        // ilspy lists nested types as `Outer.Inner` with the same dot
        // syntax as namespaces. Disambiguate by checking whether the
        // dotted prefix is itself an entity in the listing.
        let entities = vec![
            class("AchievementHandler"),
            EntityEntry {
                kind: EntityKind::Delegate,
                name: "AchievementHandler.AchievementAwarded".to_string(),
            },
        ];
        let root = build_root("x", &entities);
        // No namespace → both land under the synthetic "-" bucket.
        assert_eq!(root.children.len(), 1);
        let global = &root.children[0];
        assert_eq!(global.label, "-");
        assert_eq!(global.children.len(), 1);
        let outer = &global.children[0];
        assert_eq!(outer.label, "AchievementHandler");
        assert_eq!(outer.kind, "class");
        assert_eq!(outer.children.len(), 1);
        let inner = &outer.children[0];
        assert_eq!(inner.label, "AchievementAwarded");
        assert_eq!(inner.kind, "delegate");
        assert_eq!(inner.id, "type:AchievementHandler.AchievementAwarded");
    }

    #[test]
    fn nested_under_namespaced_outer() {
        let entities = vec![
            class("UnityEngine.UI.Button"),
            EntityEntry {
                kind: EntityKind::Enum,
                name: "UnityEngine.UI.Button.ButtonClickedEvent".to_string(),
            },
        ];
        let root = build_root("x", &entities);
        // Drill: UnityEngine → UI → Button → ButtonClickedEvent.
        let ue = &root.children[0];
        assert_eq!(ue.label, "UnityEngine");
        let ui = &ue.children[0];
        assert_eq!(ui.label, "UI");
        let button = &ui.children[0];
        assert_eq!(button.label, "Button");
        assert_eq!(button.children.len(), 1);
        assert_eq!(button.children[0].label, "ButtonClickedEvent");
        assert_eq!(button.children[0].kind, "enum");
    }
}
