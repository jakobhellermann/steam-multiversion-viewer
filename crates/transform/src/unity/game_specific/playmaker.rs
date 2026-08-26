// TODO(ai-review): review for style and correctness
//! Render a `PlayMakerFSM` component as pseudocode instead of dumping its
//! typetree.
//!
//! Object-valued parameters come out as version-stable component paths, so the
//! rendering doubles as the diff body: a moved or renamed target shows up as a
//! changed line instead of a changed `m_PathID`.

use std::path::Path;

use anyhow::{Context as _, Result};
use playmakerfsm::component::ComponentFsm;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::MonoBehaviour;

pub use playmakerfsm::component::SCRIPT_NAME;
pub use playmakerfsm::context::GameContext;

/// MIME type of the pseudocode body. No shiki grammar maps to it yet, so the
/// frontend renders it unhighlighted.
pub const MIME: &str = "text/x-playmaker-fsm";

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

/// Whether the MonoBehaviour at `path_id` is a `PlayMakerFSM`. The
/// `ClassId::MonoBehaviour` gate lives at the call site.
pub fn is_fsm<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
) -> Result<bool> {
    let script = file.object_at::<MonoBehaviour>(path_id)?.mono_script()?;
    Ok(script.is_some_and(|s| s.full_name() == SCRIPT_NAME))
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
