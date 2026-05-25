// TODO(ai-review): review for style and correctness
//! Text formatters for a single unity `SerializedFile`. Pure functions
//! that take a `SerializedFileHandle` and append into a `String` — no
//! I/O of their own beyond what `rabex_env` does when following pptrs.
//!
//! Mirrors the formatting from `rabex-env-steam-depot-vfs`'s
//! `dump_serialized` example so the on-screen output matches what you
//! get from running that binary directly.

use std::collections::BTreeMap;
use std::fmt::Write;

use anyhow::Result;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::scene_lookup::SceneLookup;
use rabex_env::unity::types::Transform;

/// Per-class object counts, sorted by class id, prefixed with a total
/// count summary. Format mirrors `dump_serialized`'s `print_class_stats`.
pub fn format_class_stats<R: EnvResolver, P: TypeTreeProvider>(
    out: &mut String,
    file: &SerializedFileHandle<'_, R, P>,
) {
    let mut counts: BTreeMap<ClassId, usize> = BTreeMap::new();
    for obj in file.file.objects() {
        *counts.entry(obj.m_ClassID).or_default() += 1;
    }

    let total: usize = counts.values().sum();
    writeln!(out, "Class stats ({total} objects):").expect("write to String");
    for (class_id, count) in &counts {
        writeln!(out, "  {count:>6}  {class_id:?}").expect("write to String");
    }
}

/// Indented gameobject/transform tree starting at the scene roots,
/// listing each gameobject's non-Transform components. Mirrors
/// `dump_serialized`'s `print_hierarchy` + `print_node`.
pub fn format_hierarchy<R: EnvResolver, P: TypeTreeProvider>(
    out: &mut String,
    file: &SerializedFileHandle<'_, R, P>,
) -> Result<()> {
    let scene = SceneLookup::new(file.file, &mut file.reader(), &file.env.tpk)?;
    for (path_id, transform) in scene.roots() {
        format_node(out, file, path_id, transform, 0)?;
    }
    Ok(())
}

fn format_node<R: EnvResolver, P: TypeTreeProvider>(
    out: &mut String,
    file: &SerializedFileHandle<'_, R, P>,
    transform_path_id: PathId,
    transform: &Transform,
    depth: usize,
) -> Result<()> {
    let go = transform
        .m_GameObject
        .deref_local(file.file, &file.env.tpk)?
        .read(&mut file.reader())?;

    let indent = "  ".repeat(depth);
    writeln!(
        out,
        "{indent}{} [{}]",
        go.m_Name, transform.m_GameObject.m_PathID
    )
    .expect("write to String");

    let child_indent = "  ".repeat(depth + 1);
    for component in &go.m_Component {
        let component_ref = component
            .component
            .deref_local::<()>(file.file, &file.env.tpk)?;
        let class_id = component_ref.info.m_ClassID;
        let path_id = component_ref.info.m_PathID;

        // Skip the transform-on-self that every gameobject has — it's
        // implied by the indentation already.
        if path_id == transform_path_id
            && matches!(class_id, ClassId::Transform | ClassId::RectTransform)
        {
            continue;
        }
        writeln!(out, "{child_indent}- {class_id:?} ({path_id})").expect("write to String");
    }

    for child_pptr in &transform.m_Children {
        let child = child_pptr
            .deref_local(file.file, &file.env.tpk)?
            .read(&mut file.reader())?;
        format_node(out, file, child_pptr.m_PathID, &child, depth + 1)?;
    }
    Ok(())
}
