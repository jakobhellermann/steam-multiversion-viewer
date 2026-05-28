// TODO(ai-review): review for style and correctness
//! Generic text-level diff helpers shared across formats.

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
