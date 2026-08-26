// TODO(ai-review): review for style and correctness
//! Dumps keyed on a widely-used Unity asset rather than on an engine
//! class: recognise a particular third-party script and dump it as
//! something readable instead of its raw serialized shape.

use anyhow::Result;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use serde_value::Value;

use super::serializedfile::dump_value::DumpOptions;

pub mod playmaker;

/// Dump the object at `path_id` with whichever game-specific dump
/// recognises it, as `(mime, body)`. `None` when none does. Which
/// objects it applies to is each dump's own decision.
pub fn try_dump<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    class_id: ClassId,
    path_id: PathId,
    opts: DumpOptions<'_>,
) -> Result<Option<(&'static str, String)>> {
    if let Some(dumped) = playmaker::try_dump(file, class_id, path_id, opts)? {
        return Ok(Some(dumped));
    }
    Ok(None)
}

/// Whether a game-specific dump considers both objects equal. `None`
/// when none is responsible or either side won't decode.
pub fn objects_equal<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    base_path_id: PathId,
    target_file: &SerializedFileHandle<'_, R, P>,
    target_path_id: PathId,
) -> Option<bool> {
    let class_id = base_file.object_at::<Value>(base_path_id).ok()?.class_id();
    if class_id
        != target_file
            .object_at::<Value>(target_path_id)
            .ok()?
            .class_id()
    {
        return None;
    }
    playmaker::objects_equal(
        base_file,
        base_path_id,
        target_file,
        target_path_id,
        class_id,
    )
}
