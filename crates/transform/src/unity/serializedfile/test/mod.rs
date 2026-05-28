// TODO(ai-review): review for style and correctness
//! Test-only support for the SerializedFile tree / diff builders.
//! `fixtures` constructs in-memory SerializedFiles via
//! [`rabex_env::rabex::files::serializedfile::builder::SerializedFileBuilder`];
//! `snapshot` runs the production tree / diff code against them with
//! insta. The `fixtures` module is `pub(crate)` so the sibling
//! `bundle::test` module can re-use `Scene` to populate archive
//! entries.

pub(crate) mod fixtures;
mod snapshot;
