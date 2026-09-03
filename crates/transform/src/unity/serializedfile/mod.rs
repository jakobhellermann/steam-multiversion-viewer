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
pub mod texture;
pub mod tree;

#[cfg(test)]
pub(crate) mod test;

use crate::structured::Node;

/// Widest a forced-spine terminal may fan out and still auto-open —
/// reveals `a/b/{c,d,e}` but not `a/b/{1..100}`.
const MAX_TERMINAL_FANOUT: usize = 6;

/// Auto-open the parts of a single-root hierarchy with nothing to choose:
/// every single-child gameobject, plus the first branching gameobject on
/// the forced spine when its fan-out is at most `MAX_TERMINAL_FANOUT`.
pub(crate) fn expand_single_child_chains(root: &mut Node) {
    open_single_child_nodes(root);
    open_forced_spine_terminal(root);
}

fn open_single_child_nodes(node: &mut Node) {
    if node.kind == "gameobject" && node.children.len() == 1 {
        node.default_collapsed = false;
    }
    for child in &mut node.children {
        open_single_child_nodes(child);
    }
}

fn open_forced_spine_terminal(node: &mut Node) {
    match node.children.as_mut_slice() {
        [only] => open_forced_spine_terminal(only),
        children
            if (2..=MAX_TERMINAL_FANOUT).contains(&children.len()) && node.kind == "gameobject" =>
        {
            node.default_collapsed = false;
        }
        _ => {}
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

    fn expanded_ids(node: &Node) -> Vec<String> {
        let mut ids = Vec::new();
        expanded(node, &mut ids);
        ids
    }

    #[test]
    fn linear_chain_opens_fully_leaf_stays_collapsed() {
        let mut root = go("a1", vec![go("a2", vec![go("a3", vec![])])]);
        expand_single_child_chains(&mut root);
        assert_eq!(
            expanded_ids(&root),
            vec!["a1".to_string(), "a2".to_string()]
        );
    }

    #[test]
    fn forced_spine_terminal_opens_when_small() {
        let mut root = go(
            "a",
            vec![go(
                "b",
                vec![go("c", vec![]), go("d", vec![]), go("e", vec![])],
            )],
        );
        expand_single_child_chains(&mut root);
        assert_eq!(expanded_ids(&root), vec!["a".to_string(), "b".to_string()],);
    }

    #[test]
    fn forced_spine_terminal_stays_collapsed_when_wide() {
        let wide: Vec<Node> = (0..100).map(|i| go(&format!("n{i}"), vec![])).collect();
        let mut root = go("a", vec![go("b", wide)]);
        expand_single_child_chains(&mut root);
        assert_eq!(expanded_ids(&root), vec!["a".to_string()]);
    }

    #[test]
    fn small_branch_off_the_spine_stays_collapsed() {
        let mut root = go(
            "a",
            vec![
                go("b1", vec![go("x", vec![]), go("y", vec![])]),
                go("b2", vec![]),
            ],
        );
        expand_single_child_chains(&mut root);
        // `a` opens as the small root branch; b1's own fan-out is off the
        // spine and stays collapsed.
        assert_eq!(expanded_ids(&root), vec!["a".to_string()]);
    }

    #[test]
    fn single_child_sub_chain_after_branch_still_opens() {
        // x is a single-child node anywhere in the tree → its sub-chain opens.
        let mut root = go(
            "a1",
            vec![go(
                "a2",
                vec![go("x", vec![go("x2", vec![])]), go("y", vec![])],
            )],
        );
        expand_single_child_chains(&mut root);
        assert_eq!(
            expanded_ids(&root),
            vec!["a1".to_string(), "a2".to_string(), "x".to_string()],
        );
    }
}
