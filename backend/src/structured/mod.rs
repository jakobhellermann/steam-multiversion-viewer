// TODO(ai-review): review for style and correctness
//! Generic recursive tree types for "structured" file views — anything
//! that has more shape than plain text. Concrete builders live in
//! per-format modules (e.g. [`crate::unity`]) and produce a
//! [`StructuredTree`] which the frontend renders with a single shared
//! tree component regardless of the source format.
//!
//! Lazy content (the "what's actually inside this node?" detail) is
//! served by a separate route keyed on the node id, so the initial
//! tree response can stay small even for files with thousands of nodes.

use std::collections::BTreeMap;

use serde::Serialize;
use utoipa::ToSchema;

/// One entry in a structured tree. Nodes are recursive — children
/// follow the same shape — and frontend-opaque: the only thing the UI
/// knows about `kind` is what icon to show.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct Node {
    /// Stable identifier inside its `StructuredTree`, used by the
    /// node-content endpoint to fetch lazy detail. Format is up to the
    /// builder; the frontend treats it as an opaque string.
    pub id: String,
    /// Human-readable label shown in the tree row.
    pub label: String,
    /// Free-form type tag — e.g. `"gameobject"`, `"component"`,
    /// `"section"`. Frontend maps known kinds to icons and ignores the
    /// rest.
    pub kind: String,
    /// Optional inline suffix (e.g. `"4 components"`) rendered to the
    /// right of the label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    /// When `true`, the frontend should leave this node collapsed by
    /// default even if its kind would normally be expanded. Used for
    /// noisy sections (e.g. class-stats) that the user rarely wants
    /// open immediately.
    #[serde(default, skip_serializing_if = "is_false")]
    pub default_collapsed: bool,
    /// When `true`, the frontend hides this node unless it matches a
    /// currently-active filter. Used for "secondary" rows whose
    /// existence is only interesting when the user searches for them
    /// — e.g. nested .NET types under their outer class.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hide_unless_matched: bool,
    /// When `true`, every descendant is visible whenever this node
    /// matches an active filter — even if the descendants themselves
    /// don't match. Set on container-shaped rows (gameobjects, …)
    /// where the user is really searching for "the thing and what's
    /// inside it" rather than just the row itself.
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_descendants_on_match: bool,
    /// Faceted attributes the frontend turns into filter chips. Each
    /// key becomes one dropdown ("class", "kind", …), each value one
    /// selectable bucket. Format-specific — the renderer doesn't
    /// interpret keys, just groups by them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub facets: BTreeMap<String, String>,
    /// Set by diff trees only — tags this node as added / removed /
    /// changed / unchanged so the frontend can colour the row. Absent
    /// on plain (non-diff) structured trees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<NodeStatus>,
    /// When `true`, the content endpoint will return a body for this
    /// node (e.g. a JSON dump, a decompiled type). False / absent on
    /// rows that exist only to group or summarise (sections,
    /// class-stat aggregates). Frontend skips the content fetch and
    /// shows a placeholder for those.
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_content: bool,
    /// Direct children. Empty for leaves.
    pub children: Vec<Node>,
}

/// Per-node diff status. Lives in [`Node`] as `Option<NodeStatus>` so
/// the same tree shape covers both the non-diff renderer (where it's
/// `None` for every row) and the diff renderer.
#[derive(Clone, Copy, Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NodeStatus {
    Unchanged,
    Changed,
    Added,
    Removed,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Node {
    /// Convenience for the (very common) "node with just an id+label+kind".
    pub fn leaf(id: impl Into<String>, label: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: kind.into(),
            badge: None,
            default_collapsed: false,
            hide_unless_matched: false,
            include_descendants_on_match: false,
            facets: BTreeMap::new(),
            status: None,
            has_content: false,
            children: Vec::new(),
        }
    }

    pub fn with_badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    pub fn with_facet(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.facets.insert(key.into(), value.into());
        self
    }

    /// Prepend `prefix` to this node's id and every descendant's. Used
    /// when a builder splices an existing per-file subtree into a
    /// larger tree (e.g. a bundle containing many SerializedFiles) so
    /// the resulting ids stay unique across the merged tree without
    /// having to thread the prefix through every builder helper.
    pub fn prefix_ids(&mut self, prefix: &str) {
        self.id = format!("{prefix}{}", self.id);
        for child in &mut self.children {
            child.prefix_ids(prefix);
        }
    }
}

/// Top-level response for `/file/structured`. `kind` lets the frontend
/// pick a renderer if it wants format-specific behaviour; for V1 the
/// shared generic renderer reads it as `"unity-serialized"` (or
/// similar) and just shows the tree.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StructuredTree {
    pub kind: String,
    pub root: Node,
}

/// Result returned by the lazy node-content endpoint.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NodeContent {
    /// MIME type the frontend uses to pick a syntax highlighter.
    pub mime: String,
    pub text: String,
}
