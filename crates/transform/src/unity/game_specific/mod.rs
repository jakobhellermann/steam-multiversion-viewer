// TODO(ai-review): review for style and correctness
//! Renderings keyed on a widely-used Unity asset rather than on an
//! engine class: recognise a particular third-party script and render it
//! as something readable instead of its raw serialized shape.

use anyhow::Result;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

use super::serializedfile::dump_value::DumpOptions;

pub mod playmaker;

/// Render the object at `path_id` with whichever game-specific
/// rendering recognises it, as `(mime, body)`. `None` when none does.
pub fn try_dump<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    class_id: ClassId,
    path_id: PathId,
    opts: DumpOptions<'_>,
) -> Result<Option<(&'static str, String)>> {
    if class_id != ClassId::MonoBehaviour {
        return Ok(None);
    }
    if playmaker::is_fsm(file, path_id)? {
        let pseudo = playmaker::dump_pseudocode(file, path_id, opts.playmaker_game)?;
        return Ok(Some((playmaker::MIME, pseudo)));
    }
    Ok(None)
}
