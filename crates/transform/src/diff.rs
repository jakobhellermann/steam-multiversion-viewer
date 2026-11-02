// TODO(ai-review): review for style and correctness
//! Generic text-level diff helpers shared across formats.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;

use similar::ChangeTag;

use crate::structured::DiffSide;

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
pub fn unified_diff_text(base: &str, target: &str, base_label: &str, target_label: &str) -> String {
    unified_diff(base, target, base_label, target_label).text
}

/// [`unified_diff_text`] plus where each side's lines landed in it.
pub struct UnifiedDiff {
    pub text: String,
    /// `(side, line index)` → byte offset of that line's content in `text`.
    /// Context lines count as `base`.
    line_starts: HashMap<(DiffSide, usize), usize>,
}

impl UnifiedDiff {
    /// Move `range`, a byte range of `side`'s text, into [`Self::text`]; `None`
    /// if its line isn't shown (outside the hunks, or a context line on `target`).
    pub fn map_range(
        &self,
        side: DiffSide,
        side_text: &LineIndex,
        range: Range<usize>,
    ) -> Option<Range<usize>> {
        let (line, line_start) = side_text.line_of(range.start);
        let start = self.line_starts.get(&(side, line))? + (range.start - line_start);
        Some(start..start + range.len())
    }
}

/// Start offsets of a text's lines.
pub struct LineIndex(Vec<usize>);

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        LineIndex(starts)
    }

    /// Index and start offset of the line holding byte `offset`.
    fn line_of(&self, offset: usize) -> (usize, usize) {
        let line = self.0.partition_point(|&start| start <= offset) - 1;
        (line, self.0[line])
    }
}

/// Mirrors `similar`'s `UnifiedDiff` formatting while recording line origins.
#[tracing::instrument(skip_all)]
pub fn unified_diff(base: &str, target: &str, base_label: &str, target_label: &str) -> UnifiedDiff {
    let diff = similar::TextDiff::from_lines(target, base);
    let mut text = String::new();
    let mut line_starts = HashMap::new();
    for (i, hunk) in diff
        .unified_diff()
        .context_radius(3)
        .iter_hunks()
        .enumerate()
    {
        if i == 0 {
            writeln!(text, "--- {target_label}").expect("write to String");
            writeln!(text, "+++ {base_label}").expect("write to String");
        }
        writeln!(text, "{}", hunk.header()).expect("write to String");
        for change in hunk.iter_changes() {
            write!(text, "{}", change.tag()).expect("write to String");
            let origin = match change.tag() {
                ChangeTag::Delete => change.old_index().map(|l| (DiffSide::Target, l)),
                ChangeTag::Insert | ChangeTag::Equal => {
                    change.new_index().map(|l| (DiffSide::Base, l))
                }
            };
            if let Some(origin) = origin {
                line_starts.insert(origin, text.len());
            }
            text.push_str(&change.to_string_lossy());
            if !diff.newline_terminated() {
                text.push('\n');
            } else if change.missing_newline() {
                text.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    UnifiedDiff { text, line_starts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_similars_formatting() {
        let target = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk";
        let base = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\nk\nl\n";
        let expected = similar::TextDiff::from_lines(target, base)
            .unified_diff()
            .context_radius(3)
            .header("t", "b")
            .to_string();
        assert_eq!(unified_diff(base, target, "b", "t").text, expected);
    }

    #[test]
    fn maps_side_ranges_onto_their_diff_lines() {
        let target = "keep Foo\nold Bar\n";
        let base = "keep Foo\nnew Baz\n";
        let diff = unified_diff(base, target, "b", "t");
        assert_eq!(
            diff.text,
            "--- t\n+++ b\n@@ -1,2 +1,2 @@\n keep Foo\n-old Bar\n+new Baz\n"
        );
        let at = |side, text: &str, word: &str| {
            let start = text.find(word).unwrap();
            let range = diff.map_range(side, &LineIndex::new(text), start..start + word.len())?;
            Some(diff.text[range].to_string())
        };
        assert_eq!(at(DiffSide::Base, base, "Foo"), Some("Foo".to_string()));
        assert_eq!(at(DiffSide::Base, base, "Baz"), Some("Baz".to_string()));
        assert_eq!(at(DiffSide::Target, target, "Bar"), Some("Bar".to_string()));
        assert_eq!(at(DiffSide::Target, target, "Foo"), None);
    }
}
