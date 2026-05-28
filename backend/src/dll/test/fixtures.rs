// TODO(ai-review): review for style and correctness
//! Small builders for [`EntityEntry`] lists. The pure
//! `dll::tree::build_root` / `dll::diff::build_root` functions take
//! an `&[EntityEntry]` directly, so we don't need to fake any
//! ilspycmd output — just hand-assemble the entity listing each test
//! wants.

use crate::dll::{EntityEntry, EntityKind};

pub(super) fn class(name: &str) -> EntityEntry {
    EntityEntry {
        kind: EntityKind::Class,
        name: name.to_owned(),
    }
}

pub(super) fn entity(kind: EntityKind, name: &str) -> EntityEntry {
    EntityEntry {
        kind,
        name: name.to_owned(),
    }
}
