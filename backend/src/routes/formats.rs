// TODO(ai-review): review for style and correctness
//! Per-format facts for the route layer.
//!
//! [`FormatCaps::deep_comparable`] must stay in sync with
//! [`super::diff::structured::deep_structured_diff`]'s dispatch: the
//! sweep calls it for every deep-comparable path, and the dispatch's
//! fallthrough panics.

use transform::Transformer;

use super::files::RichView;

#[derive(Debug, Clone, Copy)]
pub struct FormatCaps {
    /// Rich view the file preview switches to; `None` keeps the plain
    /// preview.
    pub rich_view: Option<RichView>,
    /// Whether `/file/diff` can render this format as text.
    pub text_dump: bool,
    /// Whether the manifest deep-diff sweep may structurally diff this
    /// format.
    pub deep_comparable: bool,
}

pub fn capabilities(kind: &Transformer) -> FormatCaps {
    match kind {
        Transformer::Cli(_) => FormatCaps {
            rich_view: Some(RichView::Transformed),
            text_dump: true,
            deep_comparable: false,
        },
        #[cfg(feature = "unity")]
        Transformer::UnitySerialized => FormatCaps {
            rich_view: Some(RichView::Structured),
            text_dump: true,
            deep_comparable: true,
        },
        #[cfg(feature = "unity")]
        Transformer::UnityBundle => FormatCaps {
            rich_view: Some(RichView::Structured),
            text_dump: false,
            deep_comparable: true,
        },
        Transformer::Dll => FormatCaps {
            rich_view: Some(RichView::Structured),
            text_dump: false,
            deep_comparable: false,
        },
        #[cfg(feature = "unity")]
        Transformer::AddressablesCatalog => FormatCaps {
            rich_view: Some(RichView::Structured),
            text_dump: false,
            deep_comparable: true,
        },
    }
}
