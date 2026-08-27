// TODO(ai-review): review for style and correctness
//! Test-only support for the dll tree / diff builders: `fixtures` builds
//! `EntityEntry` lists, `snapshot` insta-snapshots trees, `native` covers
//! the non-managed-PE tree. No ilspy; pure functions exercised directly.

mod fixtures;
mod native;
mod snapshot;
