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
use std::sync::Arc;

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::{GameObject, MonoBehaviour, Transform};
use rabex_env_steam_depot_vfs::SteamDepotGameFiles;
use steam_depot_vfs::chunk_store::ChunkStore;
use steam_depot_vfs::fs::DepotManifestStore;

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

/// Read the object at `path_id` and pretty-print it as JSON using the
/// typetree. Returns `(json, mime)` ready to hand back through
/// `/file/structured/node`.
pub fn dump_object_json<C: ChunkStore + 'static>(
    manifest_store: Arc<DepotManifestStore<C>>,
    path: &str,
    path_id: PathId,
) -> Result<String> {
    let game_files = SteamDepotGameFiles::new(manifest_store)?;
    let relative = path
        .strip_prefix(&format!("{}/", game_files.data_dir().display()))
        .unwrap_or(path);
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new(game_files, &tpk);
    let file = env.load_cached(relative)?;
    // Use serde_value::Value as the intermediate — unlike
    // serde_json::Value it has a Bytes variant, so non-UTF-8 string
    // fields (TextAssets that store binary blobs, savegame payloads
    // smuggled through MonoBehaviour, …) deserialize instead of failing
    // with "invalid type: byte array". serde_json then re-encodes
    // Bytes(Vec<u8>) as a JSON array of integers, matching what `jq`
    // already does for similar cases.
    let object = file.object_at::<serde_value::Value>(path_id)?;
    let value = object.read()?;
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Construct the structured tree for the file at `path` in
/// `manifest_store`. Synchronous — wrap in `spawn_blocking` from async
/// context (same as [`super::dump_unity_serialized`]).
pub fn build_tree<C: ChunkStore + 'static>(
    manifest_store: Arc<DepotManifestStore<C>>,
    path: &str,
) -> Result<StructuredTree> {
    let game_files = SteamDepotGameFiles::new(manifest_store)?;
    let relative = path
        .strip_prefix(&format!("{}/", game_files.data_dir().display()))
        .unwrap_or(path);
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new(game_files, &tpk);
    let file = env.load_cached(relative)?;
    let root = build_root_node(&file, path)?;
    Ok(StructuredTree {
        kind: TREE_KIND.to_string(),
        root,
    })
}

fn build_root_node<R: EnvResolver, P: TypeTreeProvider>(
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
    // useful at a glance and is in the node id anyway. Components
    // keep their `[pid]` suffix because the class name alone often
    // repeats within a scene.
    Ok(Node {
        id: format!("obj:{go_path_id}"),
        label: if go.m_Name.is_empty() {
            "(unnamed)".to_string()
        } else {
            go.m_Name.clone()
        },
        kind: "gameobject".to_string(),
        badge,
        default_collapsed: false,
        children,
        ..Default::default()
    })
}

// ---- loose section -------------------------------------------------------

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
/// `with_pathid_badge` is set for loose objects where the same class
/// repeats across the section (need an id to tell rows apart) and
/// cleared for components hanging under a gameobject (you rarely see
/// the same class twice on one object — the noise outweighs the info).
fn component_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
    class_id: ClassId,
    with_pathid_badge: bool,
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
    let mut node = Node::leaf(format!("obj:{path_id}"), &class_label, "component")
        .with_facet("class", &class_label);
    if with_pathid_badge {
        node = node.with_badge(format!("[{path_id}]"));
    }
    Ok(node)
}
