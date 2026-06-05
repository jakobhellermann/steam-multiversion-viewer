// TODO(ai-review): review for style and correctness
//! Unity SerializedFile reading: structured-tree (`tree`), diff
//! (`diff`), value dump (`dump_value`), text format (`format`), and
//! diff-relevant marker logic (`markers`). The sibling `bundle`
//! module re-uses these for per-archive subtrees.

pub mod diff;
pub mod dump_value;
pub mod format;
pub mod markers;
pub mod shader;
pub mod tree;

#[cfg(test)]
pub(crate) mod test;

use crate::structured::Node;

/// Open linear gameobject chains by default: a gameobject with exactly
/// one child has nothing to choose between, so leave it expanded. A run
/// of single-child gameobjects opens all the way down; expansion stops
/// at the first gameobject that branches (or at a leaf). Components and
/// section rows keep whatever the builder decided.
pub(crate) fn expand_single_child_chains(node: &mut Node) {
    if node.kind == "gameobject" && node.children.len() == 1 {
        node.default_collapsed = false;
    }
    for child in &mut node.children {
        expand_single_child_chains(child);
    }
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    fn go(id: &str, children: Vec<Node>) -> Node {
        Node {
            id: id.to_string(),
            kind: "gameobject".to_string(),
            default_collapsed: true,
            children,
            ..Default::default()
        }
    }

    /// Collect the ids of nodes left expanded (`!default_collapsed`).
    fn expanded(node: &Node, out: &mut Vec<String>) {
        if !node.default_collapsed {
            out.push(node.id.clone());
        }
        for c in &node.children {
            expanded(c, out);
        }
    }

    #[test]
    fn linear_chain_opens_fully_branch_stays_collapsed() {
        // a1 → a2 → a3(leaf) opens; b with two children stops.
        let mut root = go("a1", vec![go("a2", vec![go("a3", vec![])])]);
        expand_single_child_chains(&mut root);
        let mut ids = Vec::new();
        expanded(&root, &mut ids);
        // a1 and a2 have exactly one child → open; a3 is a leaf → stays collapsed.
        assert_eq!(ids, vec!["a1".to_string(), "a2".to_string()]);

        let mut branch = go("b", vec![go("c", vec![]), go("d", vec![])]);
        expand_single_child_chains(&mut branch);
        let mut branch_ids = Vec::new();
        expanded(&branch, &mut branch_ids);
        assert_eq!(branch_ids, Vec::<String>::new());
    }

    #[test]
    fn sub_chain_after_branch_still_opens() {
        // a1 → a2 branches to x(→x2) and y; the x sub-chain opens too.
        let mut root = go(
            "a1",
            vec![go(
                "a2",
                vec![go("x", vec![go("x2", vec![])]), go("y", vec![])],
            )],
        );
        expand_single_child_chains(&mut root);
        let mut ids = Vec::new();
        expanded(&root, &mut ids);
        // a1(1 child) and x(1 child) open; a2 branches; x2/y are leaves.
        assert_eq!(ids, vec!["a1".to_string(), "x".to_string()]);
    }
}
