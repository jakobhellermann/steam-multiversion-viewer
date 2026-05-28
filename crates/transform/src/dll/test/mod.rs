// TODO(ai-review): review for style and correctness
//! Test-only support for the dll tree / diff builders. `fixtures`
//! constructs `EntityEntry` lists; `snapshot` yaml-snapshots the
//! resulting [`StructuredTree`]s via insta. No ilspy involvement —
//! the pure `build_root` helpers are exercised directly.

mod fixtures;
mod snapshot;
