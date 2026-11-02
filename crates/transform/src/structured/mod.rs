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

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::json;
use utoipa::ToSchema;

/// Opaque identifier for a node within one structured file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl From<String> for NodeId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for NodeId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for NodeId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Corresponding node ids in one structured diff.
#[derive(Debug, Clone, Default)]
pub struct DiffNodeMatch {
    pub base: Option<NodeId>,
    pub target: Option<NodeId>,
}

/// One entry in a structured tree. Nodes are recursive.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct Node {
    /// Stable identifier inside its `StructuredTree`, used by the
    /// node-content endpoint to fetch lazy detail. Format is up to the
    /// builder; the frontend treats it as an opaque string.
    pub id: String,
    /// Version-local ids represented by this diff row. This stays
    /// backend-only; history uses it to follow an object across versions.
    #[serde(skip_serializing)]
    pub diff_match: DiffNodeMatch,
    /// Human-readable label shown in the tree row.
    pub label: String,
    /// Free-form type tag builders key decisions off internally (e.g.
    /// `"gameobject"` collapsing, bundle-diff matching) — not sent to
    /// the frontend.
    #[serde(skip_serializing)]
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
    /// Body MIME when known up front and not plain text (e.g. `"image/png"` for a texture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_mime: Option<String>,
    /// Direct children. Empty for leaves.
    // utoipa's schema collector recurses through nested ToSchema types
    // to register their schemas; without this flag it stack-overflows
    // on the self-referential `Node` → `Vec<Node>` cycle.
    #[schema(no_recursion)]
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
            diff_match: DiffNodeMatch::default(),
            label: label.into(),
            kind: kind.into(),
            badge: None,
            default_collapsed: false,
            hide_unless_matched: false,
            include_descendants_on_match: false,
            facets: BTreeMap::new(),
            status: None,
            has_content: false,
            content_mime: None,
            children: Vec::new(),
        }
    }

    pub fn with_badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    pub fn with_content_mime(mut self, mime: impl Into<String>) -> Self {
        self.content_mime = Some(mime.into());
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

/// Top-level response for `/file/structured`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(example = json!({
    "root": {
        "id": "file:level0",
        "label": "level0",
        "badge": "4 objects",
        "children": [
            {
                "id": "section:hierarchy",
                "label": "Hierarchy",
                "children": [
                    {"id": "obj:1", "label": "Player", "has_content": true, "children": []}
                ]
            }
        ]
    }
}))]
pub struct StructuredTree {
    pub root: Node,
}

impl StructuredTree {
    /// Diff trees: no actual difference (unchanged root, no children).
    pub fn has_no_changes(&self) -> bool {
        self.root.children.is_empty()
            && matches!(self.root.status, None | Some(NodeStatus::Unchanged))
    }
}

/// Result returned by the lazy node-content endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[schema(example = json!({
    "mime": "application/json",
    "text": "{\n  \"m_Name\": \"Player\"\n}"
}))]
pub struct NodeContent {
    /// MIME type the frontend uses to pick a syntax highlighter.
    pub mime: String,
    pub text: String,
    /// Spans of `text` that link to a tree node, in source order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<TextLink>,
}

/// A span of [`NodeContent::text`] linking to a structured-tree node.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TextLink {
    /// Offsets in UTF-16 code units, i.e. JavaScript string indices.
    pub start: usize,
    pub end: usize,
    /// Id of the target node, in `file`'s tree.
    pub node_id: String,
    /// Depot path of the file holding the target, `None` for this file.
    pub file: Option<String>,
    /// In a diff body: the manifest `file` and `node_id` belong to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<DiffSide>,
}

/// One manifest of a diff: `base` is shown as `+`, `target` as `-`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DiffSide {
    Base,
    Target,
}

/// Convert byte ranges of `text` to UTF-16 code unit ranges.
pub fn utf16_ranges(text: &str, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut bounds: Vec<usize> = ranges.iter().flat_map(|r| [r.start, r.end]).collect();
    bounds.sort_unstable();
    let mut converted = HashMap::with_capacity(bounds.len());
    let mut utf16 = 0;
    let mut chars = text.char_indices().peekable();
    for bound in bounds {
        while let Some(&(byte, c)) = chars.peek()
            && byte < bound
        {
            utf16 += c.len_utf16();
            chars.next();
        }
        converted.insert(bound, utf16);
    }
    ranges
        .iter()
        .map(|r| converted[&r.start]..converted[&r.end])
        .collect()
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Diff trees: drop children that came back `unchanged` with nothing
/// changed beneath them.
pub fn prune_unchanged(mut children: Vec<Node>) -> Vec<Node> {
    children.retain(|c| c.status != Some(NodeStatus::Unchanged) || !c.children.is_empty());
    children
}

/// Status for a row given its children: `changed` when any child
/// carries an explicit non-unchanged status.
pub fn aggregate_status(children: &[Node]) -> NodeStatus {
    if children
        .iter()
        .any(|c| c.status.is_some_and(|s| s != NodeStatus::Unchanged))
    {
        NodeStatus::Changed
    } else {
        NodeStatus::Unchanged
    }
}

#[cfg(test)]
mod utf16_tests {
    use super::utf16_ranges;

    #[test]
    fn utf16_ranges_count_code_units_past_multibyte_chars() {
        // "ä" is 2 bytes / 1 unit, "😀" is 4 bytes / 2 units.
        let text = "ä Foo 😀 Bar";
        let foo = text.find("Foo").unwrap();
        let bar = text.find("Bar").unwrap();
        assert_eq!(
            utf16_ranges(text, &[foo..foo + 3, bar..bar + 3]),
            vec![2..5, 9..12]
        );
    }
}
