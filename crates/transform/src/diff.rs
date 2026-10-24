// TODO(ai-review): review for style and correctness
//! Generic text-level diff helpers shared across formats.

/// One file's path on the base and on the target side of a diff; they differ where the two
/// manifests keep it under different folders (e.g. per-platform build folders).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffPaths {
    pub base: String,
    pub target: String,
}

impl DiffPaths {
    pub fn same(path: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            base: path.clone(),
            target: path,
        }
    }
}

/// Unified diff in the exact wire format the `/file/structured-diff/
/// node` route returns: `target` on the `-` side, `base` on the `+`
/// side, three lines of context. Callers pass human-readable labels
/// (depot/manifest + creation date) that land in the `--- …` /
/// `+++ …` header — same shape as `/file/diff`.
#[tracing::instrument(skip_all)]
pub fn unified_diff_text(base: &str, target: &str, base_label: &str, target_label: &str) -> String {
    similar::TextDiff::from_lines(target, base)
        .unified_diff()
        .context_radius(3)
        .header(target_label, base_label)
        .to_string()
}
