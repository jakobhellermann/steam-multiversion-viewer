// TODO(ai-review): review for style and correctness
//! Format-handling crate: takes binary input from a depot manifest
//! and renders it as text or as a [`structured::StructuredTree`].
//! Houses the per-format implementations (`dll`, `unity::*`) plus the
//! on-disk text cache (`cache`) and the dispatch (`tools`).
//!
//! Today's public surface mirrors the pre-extraction layout: callers
//! reach into `transform::dll`, `transform::unity`, etc. directly. A
//! follow-up will introduce a single `Transformer` trait that
//! consolidates per-format dispatch in one place.

pub mod cache;
pub mod diff;
pub mod dll;
pub mod structured;
pub mod tools;
#[cfg(feature = "unity")]
pub mod unity;

pub use cache::{
    ARTIFACT_MAIN, CliTool, TempInput, TransformError, Transformer, cache_artifact_path,
    read_cached, read_cached_artifact, run_and_cache, tempfile_for, write_cached_artifact,
};
pub use tools::transformer_for;
