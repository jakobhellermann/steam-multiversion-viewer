// TODO(ai-review): review for style and correctness
//! Build a [`StructuredTree`] for a unity serialized file. Mirrors the
//! sections shown by the text dump but in a renderable shape:
//!
//! ```text
//! file root
//! ├─ Class stats          (badge: total object count)
//! │  ├─ GameObject  ×420
//! │  └─ Transform   ×420
//! ├─ Hierarchy            (badge: root count)
//! │  ├─ Player [pid]      (badge: component count)
//! │  │  ├─ MeshRenderer
//! │  │  └─ <child gameobject>
//! │  └─ ...
//! └─ Loose components     (badge: count)
//!    └─ AssetBundle [pid]
//! ```
//!
//! Component labels use `MonoScript::full_name()` for MonoBehaviours so
//! later diff-matching can key on namespace + class (matching istaan's
//! `ComponentKey`) — naive `ClassId::MonoBehaviour` everywhere would
//! collapse hundreds of distinct scripts into one bucket.

use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::{GameObject, MonoBehaviour, Transform};

use crate::structured::{Node, StructuredTree};

/// Append "s" when `n != 1`. Naive but enough for the "1 component" /
/// "5 components" badges we render here.
fn pluralize(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Tree-`kind` identifier used in [`StructuredTree::kind`]. The
/// frontend keys off this to pick renderer behaviour.
pub const TREE_KIND: &str = "unity-serialized";

/// Resolve a node id (as built by [`build_tree`]) back to its
/// path-id. Returns `None` for ids that don't refer to a specific
/// object (section headers, class-stats rows).
pub fn parse_object_node_id(id: &str) -> Option<PathId> {
    id.strip_prefix("obj:").and_then(|n| n.parse().ok())
}

/// Construct the structured tree for the file at `path` (manifest-
/// relative) using a prebuilt `env`. `data_dir` is the game's data
/// directory, used to strip the prefix before handing the path to
/// `env.load_serialized` (which works in data-dir-relative paths).
/// Synchronous — wrap in `spawn_blocking` from async context.
pub fn build_tree<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    data_dir: &str,
    path: &str,
) -> Result<StructuredTree> {
    let relative = path.strip_prefix(&format!("{data_dir}/")).unwrap_or(path);
    let file = env.load_serialized(relative)?;
    let root = build_root_node(&file, path)?;
    Ok(StructuredTree {
        kind: TREE_KIND.to_string(),
        root,
    })
}

/// Build the per-file root node (class-stats / hierarchy / loose). The
/// ids are bare (`obj:N`, `section:hierarchy`, …); callers that splice
/// the result into a larger tree (e.g. a bundle) should run
/// [`Node::prefix_ids`] on the returned node before merging.
#[tracing::instrument(skip_all, fields(label))]
pub fn build_root_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    label: &str,
) -> Result<Node> {
    let stats = collect_class_stats(file);
    let hierarchy = build_hierarchy_section(file)?;
    let loose = build_loose_section(file, &hierarchy.covered)?;
    let class_stats = build_class_stats_section(&stats);

    let total_objects: usize = stats.values().sum();
    Ok(Node {
        id: format!("file:{label}"),
        label: label.to_string(),
        kind: "file".to_string(),
        badge: Some(format!(
            "{total_objects} {}",
            pluralize(total_objects, "object")
        )),
        default_collapsed: false,
        children: vec![class_stats, hierarchy.node, loose],
        ..Default::default()
    })
}

// ---- class-stats section -------------------------------------------------

#[tracing::instrument(skip_all)]
fn collect_class_stats<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
) -> BTreeMap<ClassId, usize> {
    let mut counts: BTreeMap<ClassId, usize> = BTreeMap::new();
    for obj in file.file.objects() {
        *counts.entry(obj.m_ClassID).or_default() += 1;
    }
    counts
}

fn build_class_stats_section(counts: &BTreeMap<ClassId, usize>) -> Node {
    let children: Vec<Node> = counts
        .iter()
        .map(|(class_id, count)| {
            // Reuse the same id-shape as components/objects for cheap
            // de-dup later; this row never receives lazy content.
            Node::leaf(
                format!("class:{class_id:?}"),
                format!("{class_id:?}"),
                "class-stat",
            )
            .with_badge(format!("×{count}"))
        })
        .collect();
    let total: usize = counts.values().sum();
    Node {
        id: "section:class-stats".to_string(),
        label: "Class stats".to_string(),
        kind: "section".to_string(),
        badge: Some(format!("{total} {}", pluralize(total, "object"))),
        // Long noisy list — collapse it so the user has to opt in.
        default_collapsed: true,
        children,
        ..Default::default()
    }
}

// ---- hierarchy section ---------------------------------------------------

struct HierarchySection {
    node: Node,
    /// Path ids visited while building the hierarchy — gameobjects,
    /// their transforms, and the components they own. Used by the
    /// loose-section pass to compute "everything else".
    covered: HashSet<PathId>,
}

#[tracing::instrument(skip_all)]
fn build_hierarchy_section<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
) -> Result<HierarchySection> {
    // Collect transforms first so we can walk roots and traverse
    // children without redoing the deref dance for every visit.
    let mut transforms_by_path: BTreeMap<PathId, (Transform, GameObject)> = BTreeMap::new();
    for handle in file.transforms() {
        let path_id = handle.path_id();
        let transform = handle.read()?;
        let go = file.deref(transform.m_GameObject)?.read()?;
        transforms_by_path.insert(path_id, (transform, go));
    }

    let mut covered: HashSet<PathId> = HashSet::new();
    let mut roots: Vec<Node> = Vec::new();
    for (root_path_id, (transform, go)) in &transforms_by_path {
        if !transform.m_Father.is_null() {
            continue;
        }
        let node = build_gameobject_node(
            file,
            &transforms_by_path,
            &mut covered,
            *root_path_id,
            transform,
            go,
        )?;
        roots.push(node);
    }

    let root_count = roots.len();
    // A hierarchy with a single root has no sibling to choose between,
    // so open its single-child chain by default. With several roots we
    // leave everything collapsed rather than spotlight one chain.
    if let [only] = roots.as_mut_slice() {
        super::expand_single_child_chains(only);
    }
    Ok(HierarchySection {
        node: Node {
            id: "section:hierarchy".to_string(),
            label: "Hierarchy".to_string(),
            kind: "section".to_string(),
            badge: Some(format!("{root_count} {}", pluralize(root_count, "root"))),
            default_collapsed: false,
            children: roots,
            ..Default::default()
        },
        covered,
    })
}

fn build_gameobject_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    transforms_by_path: &BTreeMap<PathId, (Transform, GameObject)>,
    covered: &mut HashSet<PathId>,
    transform_path_id: PathId,
    transform: &Transform,
    go: &GameObject,
) -> Result<Node> {
    let go_path_id = transform.m_GameObject.m_PathID;
    covered.insert(go_path_id);
    covered.insert(transform_path_id);

    let mut children: Vec<Node> = Vec::new();

    // Components first (skipping the transform-on-self that every
    // gameobject has — it's implied by the row's position in the tree).
    for component in &go.m_Component {
        let component_ref = component
            .component
            .deref_local::<()>(file.file, &file.env.tpk)?;
        let class_id = component_ref.info.m_ClassID;
        let path_id = component_ref.info.m_PathID;
        if path_id == transform_path_id
            && matches!(class_id, ClassId::Transform | ClassId::RectTransform)
        {
            // Still mark covered so the loose section doesn't list it.
            covered.insert(path_id);
            continue;
        }
        covered.insert(path_id);
        children.push(component_node(file, path_id, class_id, false)?);
    }

    // Then child gameobjects (via transform children).
    for child_pptr in &transform.m_Children {
        let child_path_id = child_pptr.m_PathID;
        if let Some((child_transform, child_go)) = transforms_by_path.get(&child_path_id) {
            children.push(build_gameobject_node(
                file,
                transforms_by_path,
                covered,
                child_path_id,
                child_transform,
                child_go,
            )?);
        }
    }

    let component_count = go.m_Component.len().saturating_sub(1);
    let badge = (component_count > 0).then(|| {
        format!(
            "{component_count} {}",
            pluralize(component_count, "component")
        )
    });

    // Gameobject label is just the name — the path id is rarely
    // useful at a glance and is in the node id anyway.
    Ok(Node {
        id: format!("obj:{go_path_id}"),
        label: if go.m_Name.is_empty() {
            "(unnamed)".to_string()
        } else {
            go.m_Name.clone()
        },
        kind: "gameobject".to_string(),
        badge,
        // Collapsed by default so the hierarchy opens as a root list to
        // drill into, not a fully-expanded scene. Search still expands
        // matches via the frontend.
        default_collapsed: true,
        // Searching for a gameobject usually means "show me everything
        // about it" — its components + child gameobjects too. The
        // frontend uses this flag to splice descendants into the
        // visible set when the gameobject itself matches.
        include_descendants_on_match: true,
        has_content: true,
        children,
        ..Default::default()
    })
}

// ---- loose section -------------------------------------------------------

#[tracing::instrument(skip_all)]
fn build_loose_section<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    covered: &HashSet<PathId>,
) -> Result<Node> {
    let mut children: Vec<Node> = Vec::new();
    for obj in file.file.objects() {
        let path_id = obj.m_PathID;
        if covered.contains(&path_id) {
            continue;
        }
        children.push(component_node(file, path_id, obj.m_ClassID, true)?);
    }
    let count = children.len();
    Ok(Node {
        id: "section:loose".to_string(),
        label: "Loose components".to_string(),
        kind: "section".to_string(),
        badge: Some(format!("{count}")),
        default_collapsed: false,
        children,
        ..Default::default()
    })
}

// ---- shared component-node helper ----------------------------------------

/// Build a leaf node for a single component / loose object. Uses
/// `MonoScript::full_name()` for MonoBehaviours so the label carries
/// the actual script name instead of the generic `MonoBehaviour` class.
///
/// `loose` is set for top-level objects: they're labelled by `m_Name`
/// and carry a type badge so the class stays recognizable (the
/// hierarchy shows the class directly, but a loose object's name alone
/// doesn't). Cleared for components hanging under a gameobject.
#[tracing::instrument(level = "debug", skip_all, fields(?class_id, ?path_id))]
fn component_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
    class_id: ClassId,
    loose: bool,
) -> Result<Node> {
    // `class` facet drives the frontend filter. For MonoBehaviours
    // that's the script name (user-mental "the class"), for everything
    // else the engine class id. Falls back to the engine class id when
    // the script reference is missing.
    let class_label = if matches!(class_id, ClassId::MonoBehaviour) {
        let handle = file.object_at::<MonoBehaviour>(path_id)?;
        match handle.mono_script()? {
            Some(script) => script.full_name().into_owned(),
            None => format!("{class_id:?}"),
        }
    } else {
        format!("{class_id:?}")
    };
    // For loose objects we additionally pull `m_Name` to label assets
    // by their author-set name — e.g. a `MonoScript` row reads
    // `PlayerController` instead of the bare engine class. Components
    // under a gameobject keep the class label: their m_Name is almost
    // always empty and the hierarchy position already disambiguates.
    //
    // MonoBehaviours stay on `class_label` because `full_name()` is
    // already the user-mental identity, and MB's own `m_Name` is
    // typically empty.
    let display_label = if loose && !matches!(class_id, ClassId::MonoBehaviour) {
        read_m_name(file, path_id)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| class_label.clone())
    } else {
        class_label.clone()
    };
    let mut node = Node::leaf(format!("obj:{path_id}"), &display_label, "component")
        .with_facet("class", &class_label);
    node.has_content = true;
    if loose && display_label != class_label {
        node = node.with_badge(class_label.clone());
    }
    if matches!(class_id, ClassId::Shader) {
        node.children = shader_children(file, path_id);
        node.default_collapsed = true;
    }
    Ok(node)
}

/// Per-platform → per-program child nodes for a `Shader`, from
/// `m_ParsedForm` (no blob decompression). Each program leaf's source is
/// served lazily by the content endpoint via its `prog:` id. Best-effort
/// — an unreadable shader just gets no children.
fn shader_children<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
) -> Vec<Node> {
    use serde_value::Value;

    let Ok(value) = file.object_at::<Value>(path_id).and_then(|h| h.read()) else {
        return Vec::new();
    };
    shader_nodes(path_id, super::shader::program_groups(&value))
}

/// Assemble the platform → (pass →) program node tree from decoded
/// program groups. Split out from the I/O so it can be snapshot-tested.
fn shader_nodes(path_id: PathId, groups: Vec<super::shader::PlatformPrograms>) -> Vec<Node> {
    groups
        .into_iter()
        .map(|group| {
            let total: usize = group.passes.iter().map(|p| p.programs.len()).sum();
            // A single pass adds no information — inline its programs;
            // multiple passes get a grouping level so identical
            // stage/keyword variants stay distinguishable.
            let children = if group.passes.len() == 1 {
                program_leaves(path_id, group.platform, &group.passes[0].programs)
            } else {
                group
                    .passes
                    .iter()
                    .enumerate()
                    .map(|(i, pass)| {
                        let leaves = program_leaves(path_id, group.platform, &pass.programs);
                        Node {
                            id: format!("obj:{path_id}/passgroup:{}:{i}", group.platform),
                            label: pass.label.clone(),
                            kind: "shader-pass".to_string(),
                            badge: Some(pass.programs.len().to_string()),
                            default_collapsed: true,
                            children: leaves,
                            ..Default::default()
                        }
                    })
                    .collect()
            };
            Node {
                id: format!("obj:{path_id}/plat:{}", group.platform),
                label: super::shader::platform_name(group.platform).to_string(),
                kind: "shader-platform".to_string(),
                badge: Some(total.to_string()),
                default_collapsed: true,
                children,
                ..Default::default()
            }
        })
        .collect()
}

/// Leaf nodes for a pass's programs — label is stage + keyword set, the
/// `prog:` id drives lazy source loading.
fn program_leaves(
    path_id: PathId,
    platform: u32,
    programs: &[super::shader::ProgramRef],
) -> Vec<Node> {
    programs
        .iter()
        .map(|p| {
            let label = if p.keywords.is_empty() {
                p.stage.to_string()
            } else {
                format!("{} · {}", p.stage, p.keywords.join(", "))
            };
            let mut leaf = Node::leaf(
                format!("obj:{path_id}/prog:{platform}:{}", p.blob_index),
                label,
                "shader-program",
            )
            .with_facet("stage", p.stage)
            .with_badge(p.type_name);
            leaf.has_content = true;
            leaf
        })
        .collect()
}

/// Best-effort read of `m_Name` via the dynamic value path — failure
/// (missing field, unreadable type tree) returns `None` and the
/// caller falls back to the class label.
fn read_m_name<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
) -> Option<String> {
    use serde_value::Value;
    let value: Value = file.object_at::<Value>(path_id).ok()?.read().ok()?;
    let Value::Map(map) = value else {
        return None;
    };
    match map.get(&Value::String("m_Name".to_string()))? {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unity::serializedfile::shader::{PassPrograms, PlatformPrograms, ProgramRef};

    fn program(blob_index: u32, stage: &'static str, keywords: &[&str]) -> ProgramRef {
        ProgramRef {
            blob_index,
            type_name: "GLCore32",
            stage,
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn shader_nodes_group_multi_pass_inline_single() {
        let groups = vec![
            // Multiple passes → a grouping level per pass.
            PlatformPrograms {
                platform: 15,
                passes: vec![
                    PassPrograms {
                        label: "Pass 0".to_string(),
                        programs: vec![
                            program(2, "vertex", &[]),
                            program(3, "vertex", &["USE_MASK"]),
                        ],
                    },
                    PassPrograms {
                        label: "Pass 1".to_string(),
                        programs: vec![program(4, "vertex", &[])],
                    },
                ],
            },
            // Single pass → programs inlined directly under the platform.
            PlatformPrograms {
                platform: 18,
                passes: vec![PassPrograms {
                    label: "Pass 0".to_string(),
                    programs: vec![program(7, "fragment", &[])],
                }],
            },
        ];

        insta::assert_yaml_snapshot!(shader_nodes(42, groups));
    }
}
