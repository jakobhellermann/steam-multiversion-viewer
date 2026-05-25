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

use serde::Serialize;
use utoipa::ToSchema;

/// One entry in a structured tree. Nodes are recursive — children
/// follow the same shape — and frontend-opaque: the only thing the UI
/// knows about `kind` is what icon to show.
#[derive(Debug, Clone, Serialize, ToSchema)]
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
    /// Direct children. Empty for leaves.
    pub children: Vec<Node>,
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
            children: Vec::new(),
        }
    }

    pub fn with_badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    pub fn collapsed_by_default(mut self) -> Self {
        self.default_collapsed = true;
        self
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
