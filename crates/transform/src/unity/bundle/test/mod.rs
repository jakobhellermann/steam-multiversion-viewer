// TODO(ai-review): review for style and correctness
//! Test-only support for the bundle tree / diff builders. `fixtures`
//! assembles `.unity3d`-shaped byte streams in memory (using
//! `Scene` from the sibling `serializedfile::test` module to fill
//! SerializedFile entries); `snapshot` exercises
//! [`super::build_tree_from_bundle`] and
//! [`super::build_diff_from_bundles`] with insta.

mod fixtures;
mod snapshot;
