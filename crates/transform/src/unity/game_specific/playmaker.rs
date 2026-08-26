// TODO(ai-review): review for style and correctness
//! Render a `PlayMakerFSM` component as pseudocode instead of dumping its
//! typetree.
//!
//! Object-valued parameters come out as version-stable component paths, so the
//! dump doubles as the diff body: a moved or renamed target shows up as a
//! changed line instead of a changed `m_PathID`.

use std::path::Path;

use anyhow::{Context as _, Result};
use playmakerfsm::component::ComponentFsm;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::MonoBehaviour;

use crate::unity::serializedfile::dump_value::DumpOptions;

use playmakerfsm::component::SCRIPT_NAME;

pub use playmakerfsm::context::GameContext;

/// MIME type of the pseudocode body. No shiki grammar maps to it yet, so the
/// frontend renders it unhighlighted.
const MIME: &str = "text/x-playmaker-fsm";

/// Read what this game knows beyond its FSM data, out of its managed
/// assemblies. `Assembly-CSharp.dll` holds all but a handful of a game's
/// action classes, and every further assembly is another download, so the
/// viewer settles for it: an action class it misses keeps its enum params
/// numeric.
pub fn read_game_context<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
) -> Result<GameContext> {
    let read = |name: &str| {
        env.game_files
            .read_path(&Path::new("Managed").join(name))
            .with_context(|| format!("Managed/{name}"))
    };
    let playmaker = read("PlayMaker.dll")?;
    let assembly_csharp = read("Assembly-CSharp.dll")?;
    GameContext::new(
        playmaker.as_ref(),
        &[("Assembly-CSharp.dll", assembly_csharp.as_ref())],
        playmakerfsm::context::layer_names(env)?,
    )
}

/// Supplies the name tables on demand. Reading them parses the managed
/// assemblies in full, so a dump only pays that cost — and only fails on
/// a game that has no such assemblies — once it actually renders an FSM.
pub trait GameContextSource {
    fn game_context(&self) -> Result<&GameContext>;
}

fn is_fsm<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
) -> Result<bool> {
    let script = file.object_at::<MonoBehaviour>(path_id)?.mono_script()?;
    Ok(script.is_some_and(|s| s.full_name() == SCRIPT_NAME))
}

/// Whether both objects are equal `PlayMakerFSM`s. Skips the name
/// tables: they change what the text says, not whether the two agree.
pub fn objects_equal<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    base_path_id: PathId,
    target_file: &SerializedFileHandle<'_, R, P>,
    target_path_id: PathId,
    class_id: ClassId,
) -> Option<bool> {
    if class_id != ClassId::MonoBehaviour {
        return None;
    }
    if !is_fsm(base_file, base_path_id).ok()? || !is_fsm(target_file, target_path_id).ok()? {
        return None;
    }
    let base = dump_pseudocode(base_file, base_path_id, None).ok()?;
    let target = dump_pseudocode(target_file, target_path_id, None).ok()?;
    Some(base == target)
}

/// Dump the object at `path_id` as pseudocode if it is a `PlayMakerFSM`,
/// `None` for anything else.
pub fn try_dump<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    class_id: ClassId,
    path_id: PathId,
    opts: DumpOptions<'_>,
) -> Result<Option<(&'static str, String)>> {
    if class_id != ClassId::MonoBehaviour || !is_fsm(file, path_id)? {
        return Ok(None);
    }
    let pseudo = dump_pseudocode(file, path_id, opts.playmaker_game)?;
    Ok(Some((MIME, pseudo)))
}

/// Decode the `PlayMakerFSM` at `path_id` and render it as pseudocode.
/// Without a `game`, enum and layer params stay bare integers.
pub fn dump_pseudocode<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
    game: Option<&dyn GameContextSource>,
) -> Result<String> {
    let component = ComponentFsm::read(file, path_id)?;
    let mut model = component.decode(file)?;
    if let Some(game) = game {
        game.game_context()?.apply(&mut model);
    }
    Ok(playmakerfsm::pseudo::render(&model))
}
