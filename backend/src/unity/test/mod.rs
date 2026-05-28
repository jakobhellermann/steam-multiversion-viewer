// TODO(ai-review): review for style and correctness
//! Test-only support for unity tree / diff snapshots. `fixtures`
//! builds in-memory SerializedFiles via `SerializedFileBuilder`;
//! `snapshot` exercises the production tree / diff code against them
//! with insta.

pub(super) mod fixtures;
mod snapshot;
