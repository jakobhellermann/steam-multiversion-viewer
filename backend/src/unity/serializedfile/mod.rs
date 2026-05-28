// TODO(ai-review): review for style and correctness
//! Unity SerializedFile reading: structured-tree (`tree`), diff
//! (`diff`), value dump (`dump_value`), text format (`format`), and
//! diff-relevant marker logic (`markers`). The sibling `bundle`
//! module re-uses these for per-archive subtrees.

pub mod diff;
pub mod dump_value;
pub mod format;
pub mod markers;
pub mod tree;

#[cfg(test)]
pub(crate) mod test;
