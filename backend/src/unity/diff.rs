// TODO(ai-review): review for style and correctness
//! Structured diff between two SerializedFiles. Renders a tree of
//! sections (class-stats / hierarchy / loose) where every node is
//! tagged `added` / `removed` / `changed` / `unchanged`, with
//! unchanged subtrees pruned so the response carries only the spine
//! to every difference.
//!
//! Matching is content-aware, not path-id-aware: Unity path-ids
//! shuffle across patches even when the underlying scene barely
//! changes, so a naive `obj:<pathid>` join produces noise. Instead we
//! match objects by what users actually think of as "the same thing":
//!
//! - **Hierarchy**: paired walk by gameobject name + sibling index.
//!   Components on a matched gameobject pair up by `ComponentKey`
//!   (engine class id, or `MonoScript::full_name()` for MBs).
//! - **Loose**: same `(class_id_or_script, m_Name)` key, falling
//!   back to path-id for unnamed objects (which usually means a
//!   single instance of the class is loose anyway).
//! - **Class-stats**: per-class object counts; intrinsically stable.
//!
//! Byte-level change detection on a matched pair compares the raw
//! on-disk slice (offset+size from `ObjectInfo`) — typetree-faithful
//! and skips deserialisation entirely.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::files::serializedfile::ObjectInfo;
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

use crate::structured::{Node, NodeStatus, StructuredTree};

use super::tree::TREE_KIND;

/// Per-file index: path-id → on-disk object bytes. Built once per side
/// so component/loose body comparisons stay O(1) instead of scanning
/// the object list per lookup.
type BodyIndex<'a> = HashMap<PathId, &'a [u8]>;

#[tracing::instrument(skip_all)]
fn build_body_index<'a, R: EnvResolver, P>(file: &SerializedFileHandle<'a, R, P>) -> BodyIndex<'a> {
    file.file
        .objects()
        .map(|o| (o.m_PathID, object_bytes(file, o)))
        .collect()
}

/// Build a status-tagged node with sensible defaults for the diff
/// builders — `Node` has several fields we never set here
/// (`default_collapsed`, `hide_unless_matched`,
/// `include_descendants_on_match`) but we want to be explicit about
/// status, not blanket-default it.
fn make_node(
    id: impl Into<String>,
    label: impl Into<String>,
    kind: impl Into<String>,
    status: NodeStatus,
) -> Node {
    let kind = kind.into();
    // Object-shaped rows (gameobjects, individual components) carry
    // a per-node body the diff content endpoint can render; section
    // / class-stat aggregates don't.
    let has_content = matches!(kind.as_str(), "gameobject" | "component");
    Node {
        id: id.into(),
        label: label.into(),
        kind,
        status: Some(status),
        has_content,
        ..Default::default()
    }
}

/// For matched object pairs: build a node id from the base path-id
/// and, when the target path-id differs, the target_id to thread
/// through to the per-side content lookup.
fn obj_id_pair(base: PathId, target: PathId) -> (String, Option<String>) {
    let base_id = format!("obj:{base}");
    let target_id = (base != target).then(|| format!("obj:{target}"));
    (base_id, target_id)
}

/// Build the structured diff for `path` between the two manifests.
/// Synchronous; callers from async context must wrap in
/// `tokio::task::spawn_blocking`.
#[tracing::instrument(skip_all, fields(path))]
pub fn build_diff<C: ChunkStore + 'static>(
    base_manifest: Arc<DepotManifestStore<C>>,
    target_manifest: Arc<DepotManifestStore<C>>,
    path: &str,
) -> Result<StructuredTree> {
    let base = open_side(base_manifest, path).context("base side")?;
    let target = open_side(target_manifest, path).context("target side")?;

    let class_stats = diff_class_stats(&base, &target);
    let (hierarchy, covered) = diff_hierarchy(&base, &target)?;
    let loose = diff_loose(&base, &target, &covered)?;

    let children = vec![class_stats, hierarchy, loose];
    let status = aggregate_status(&children);
    let root = Node {
        id: format!("file:{path}"),
        label: path.to_string(),
        kind: "file".to_string(),
        badge: None,
        status: Some(status),
        children,
        ..Default::default()
    };
    Ok(StructuredTree {
        kind: TREE_KIND.to_string(),
        root,
    })
}

/// One side opened for diffing: the path's data dir prefix (so we can
/// stash it next to the file handle) plus everything `SerializedFile`
/// needs.
struct OpenedSide<R: EnvResolver, P: TypeTreeProvider> {
    env: Environment<R, P>,
    relative: String,
}

#[tracing::instrument(skip_all)]
fn open_side<C: ChunkStore + 'static>(
    manifest: Arc<DepotManifestStore<C>>,
    path: &str,
) -> Result<OpenedSide<SteamDepotGameFiles<C>, TypeTreeCache<TpkTypeTreeBlob>>> {
    let game_files = SteamDepotGameFiles::new(manifest)?;
    let relative = path
        .strip_prefix(&format!("{}/", game_files.data_dir().display()))
        .unwrap_or(path)
        .to_owned();
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new(game_files, tpk);
    Ok(OpenedSide { env, relative })
}

// ---- class-stats ---------------------------------------------------------

/// Per-class object counts. Class identity is stable across patches —
/// when a class appears on one side only or its count changes, that's
/// the diff.
#[tracing::instrument(skip_all)]
fn diff_class_stats<R: EnvResolver, P: TypeTreeProvider>(
    base: &OpenedSide<R, P>,
    target: &OpenedSide<R, P>,
) -> Node {
    let b = load_file(base);
    let t = load_file(target);
    let (base_counts, base_total) = match b {
        Ok(ref f) => count_classes(f),
        Err(_) => (BTreeMap::new(), 0),
    };
    let (target_counts, target_total) = match t {
        Ok(ref f) => count_classes(f),
        Err(_) => (BTreeMap::new(), 0),
    };

    let mut all_classes: BTreeMap<ClassId, ()> = BTreeMap::new();
    for k in base_counts.keys().chain(target_counts.keys()) {
        all_classes.insert(*k, ());
    }

    let mut children: Vec<Node> = Vec::new();
    for class_id in all_classes.keys() {
        let b = base_counts.get(class_id).copied();
        let t = target_counts.get(class_id).copied();
        let (status, badge) = match (b, t) {
            (Some(bc), None) => (NodeStatus::Added, Some(format!("×{bc}"))),
            (None, Some(tc)) => (NodeStatus::Removed, Some(format!("×{tc}"))),
            (Some(bc), Some(tc)) if bc == tc => (NodeStatus::Unchanged, Some(format!("×{bc}"))),
            (Some(bc), Some(tc)) => (NodeStatus::Changed, Some(format!("×{tc} → ×{bc}"))),
            (None, None) => unreachable!(),
        };
        children.push(Node {
            badge,
            ..make_node(
                format!("class:{class_id:?}"),
                format!("{class_id:?}"),
                "class-stat",
                status,
            )
        });
    }

    let pruned: Vec<Node> = children
        .into_iter()
        .filter(|c| c.status != Some(NodeStatus::Unchanged))
        .collect();
    let status = if pruned.is_empty() && base_total == target_total {
        NodeStatus::Unchanged
    } else {
        NodeStatus::Changed
    };

    Node {
        badge: Some(if base_total == target_total {
            format!("{base_total} {}", pluralize(base_total, "object"))
        } else {
            format!(
                "{target_total} {} → {base_total} {}",
                pluralize(target_total, "object"),
                pluralize(base_total, "object")
            )
        }),
        children: pruned,
        // Class-stats is a counts-table — useful to drill into when
        // hunting for a specific class id change, noisy when reading
        // the diff top-down. Collapsed by default, same as the
        // non-diff structured view.
        default_collapsed: true,
        ..make_node("section:class-stats", "Class stats", "section", status)
    }
}

fn count_classes<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
) -> (BTreeMap<ClassId, usize>, usize) {
    let mut counts: BTreeMap<ClassId, usize> = BTreeMap::new();
    for obj in file.file.objects() {
        *counts.entry(obj.m_ClassID).or_default() += 1;
    }
    let total = counts.values().sum();
    (counts, total)
}

// ---- hierarchy -----------------------------------------------------------

/// Per-side hierarchy index: gameobject path-id → (transform, go),
/// keyed by transform path-id (matching `tree::build_hierarchy_section`'s
/// convention).
type Transforms = BTreeMap<PathId, (Transform, GameObject)>;

/// Hierarchy section diff. Pairs roots, then descendants, by
/// gameobject name + sibling-index — the same disambig istaan-diff
/// uses, since path-ids re-shuffle across patches even when the scene
/// is structurally identical. Returns the section node plus the set
/// of path-ids "covered" on each side so the loose section can skip
/// what we already accounted for.
#[tracing::instrument(skip_all)]
fn diff_hierarchy<R: EnvResolver, P: TypeTreeProvider>(
    base: &OpenedSide<R, P>,
    target: &OpenedSide<R, P>,
) -> Result<(Node, Covered)> {
    let base_file = load_file(base)?;
    let target_file = load_file(target)?;
    let base_bodies = build_body_index(&base_file);
    let target_bodies = build_body_index(&target_file);

    let base_transforms = collect_transforms(&base_file)?;
    let target_transforms = collect_transforms(&target_file)?;

    let mut covered = Covered::default();

    let base_roots = roots_with_data(&base_transforms);
    let target_roots = roots_with_data(&target_transforms);

    let (matches, unmatched_target) =
        pair_by_key(&base_roots, &target_roots, |(_, _, go)| go.m_Name.clone());

    let children = {
        let _span = tracing::info_span!("walk_roots", roots = matches.len()).entered();
        let mut children: Vec<Node> = Vec::new();
        for (bi, ti) in matches {
            let (b_id, b_t, b_go) = base_roots[bi];
            match ti {
                Some(ti) => {
                    let (t_id, t_t, t_go) = target_roots[ti];
                    children.push(walk_pair(
                        &base_file,
                        &target_file,
                        &base_bodies,
                        &target_bodies,
                        &base_transforms,
                        &target_transforms,
                        (b_id, b_t, b_go),
                        (t_id, t_t, t_go),
                        &mut covered,
                    )?);
                }
                None => {
                    children.push(subtree_one_side(
                        &base_file,
                        &base_transforms,
                        b_id,
                        b_t,
                        b_go,
                        NodeStatus::Added,
                        &mut covered.base,
                    )?);
                }
            }
        }
        for ti in unmatched_target {
            let (t_id, t_t, t_go) = target_roots[ti];
            children.push(subtree_one_side(
                &target_file,
                &target_transforms,
                t_id,
                t_t,
                t_go,
                NodeStatus::Removed,
                &mut covered.target,
            )?);
        }
        children
    };

    let base_root_count = base_roots.len();
    let target_root_count = target_roots.len();
    let badge = if base_root_count == target_root_count {
        Some(format!(
            "{base_root_count} {}",
            pluralize(base_root_count, "root")
        ))
    } else {
        Some(format!(
            "{target_root_count} {} → {base_root_count} {}",
            pluralize(target_root_count, "root"),
            pluralize(base_root_count, "root")
        ))
    };

    let pruned = prune_unchanged(children);
    let status = aggregate_status(&pruned);

    Ok((
        Node {
            badge,
            children: pruned,
            ..make_node("section:hierarchy", "Hierarchy", "section", status)
        },
        covered,
    ))
}

/// Path-ids that the hierarchy walk consumed on each side. The loose
/// section uses the complement.
#[derive(Default)]
struct Covered {
    base: HashSet<PathId>,
    target: HashSet<PathId>,
}

#[tracing::instrument(skip_all)]
fn collect_transforms<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
) -> Result<Transforms> {
    let mut transforms: Transforms = BTreeMap::new();
    for handle in file.transforms() {
        let path_id = handle.path_id();
        let transform = handle.read()?;
        let go = file.deref(transform.m_GameObject)?.read()?;
        transforms.insert(path_id, (transform, go));
    }
    Ok(transforms)
}

/// Roots with their `(Transform, GameObject)` references already
/// resolved — same shape as [`ordered_children`] so both the
/// root-pairing and child-pairing call sites can share
/// [`pair_by_key`].
fn roots_with_data(transforms: &Transforms) -> Vec<(PathId, &Transform, &GameObject)> {
    transforms
        .iter()
        .filter(|(_, (t, _))| t.m_Father.is_null())
        .map(|(id, (t, go))| (*id, t, go))
        .collect()
}

/// Pair two ordered sequences by `key`, disambiguating duplicate keys
/// by sibling-index (Nth occurrence on base → Nth still-unconsumed
/// occurrence on target). Returns matches in base order, with `None`
/// for base items that found no partner, and the indices of unmatched
/// target items in their original order.
fn pair_by_key<T, K, F>(
    base: &[T],
    target: &[T],
    key: F,
) -> (Vec<(usize, Option<usize>)>, Vec<usize>)
where
    K: Eq + std::hash::Hash + Clone,
    F: Fn(&T) -> K,
{
    let target_keys: Vec<K> = target.iter().map(&key).collect();
    let mut consumed: HashSet<usize> = HashSet::new();
    let mut seen: HashMap<K, usize> = HashMap::new();
    let mut matches: Vec<(usize, Option<usize>)> = Vec::with_capacity(base.len());
    for (bi, b) in base.iter().enumerate() {
        let k = key(b);
        let nth = {
            let entry = seen.entry(k.clone()).or_default();
            let n = *entry;
            *entry += 1;
            n
        };
        let ti = target_keys
            .iter()
            .enumerate()
            .filter(|(ti, tk)| **tk == k && !consumed.contains(ti))
            .map(|(ti, _)| ti)
            .nth(nth);
        if let Some(ti) = ti {
            consumed.insert(ti);
        }
        matches.push((bi, ti));
    }
    let unmatched: Vec<usize> = (0..target.len())
        .filter(|i| !consumed.contains(i))
        .collect();
    (matches, unmatched)
}

/// Walk two matched gameobjects in lockstep: components diff, then
/// descendants diff (recursive on this function).
#[allow(clippy::too_many_arguments)]
fn walk_pair<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    target_file: &SerializedFileHandle<'_, R, P>,
    base_bodies: &BodyIndex<'_>,
    target_bodies: &BodyIndex<'_>,
    base_transforms: &Transforms,
    target_transforms: &Transforms,
    base: (PathId, &Transform, &GameObject),
    target: (PathId, &Transform, &GameObject),
    covered: &mut Covered,
) -> Result<Node> {
    let (b_tid, b_t, b_go) = base;
    let (t_tid, t_t, t_go) = target;

    covered.base.insert(b_tid);
    covered.base.insert(b_t.m_GameObject.m_PathID);
    covered.target.insert(t_tid);
    covered.target.insert(t_t.m_GameObject.m_PathID);

    let mut children: Vec<Node> = Vec::new();

    // Components — keyed by ComponentKey so MBs collapse by script
    // name, engine components by class id. Transform-on-self is
    // skipped on both sides since it's implied by the row.
    let base_components = collect_components(base_file, b_go, b_tid, &mut covered.base)?;
    let target_components = collect_components(target_file, t_go, t_tid, &mut covered.target)?;

    let mut keys: BTreeMap<ComponentKey, ()> = BTreeMap::new();
    for k in base_components.keys().chain(target_components.keys()) {
        keys.insert(k.clone(), ());
    }
    for key in keys.keys() {
        let b = base_components.get(key);
        let t = target_components.get(key);
        children.push(component_diff_node(
            base_file,
            target_file,
            base_bodies,
            target_bodies,
            key,
            b,
            t,
        )?);
    }

    // Recurse into transform children, matched by gameobject name +
    // sibling index per side.
    let base_children = ordered_children(b_t, base_transforms);
    let target_children = ordered_children(t_t, target_transforms);

    let (matches, unmatched_target) =
        pair_by_key(&base_children, &target_children, |(_, _, go)| {
            go.m_Name.clone()
        });

    for (bi, ti) in matches {
        let (b_child_tid, b_child_t, b_child_go) = base_children[bi];
        match ti {
            Some(ti) => {
                let (t_child_tid, t_child_t, t_child_go) = target_children[ti];
                children.push(walk_pair(
                    base_file,
                    target_file,
                    base_bodies,
                    target_bodies,
                    base_transforms,
                    target_transforms,
                    (b_child_tid, b_child_t, b_child_go),
                    (t_child_tid, t_child_t, t_child_go),
                    covered,
                )?);
            }
            None => {
                children.push(subtree_one_side(
                    base_file,
                    base_transforms,
                    b_child_tid,
                    b_child_t,
                    b_child_go,
                    NodeStatus::Added,
                    &mut covered.base,
                )?);
            }
        }
    }
    for ti in unmatched_target {
        let (t_child_tid, t_child_t, t_child_go) = target_children[ti];
        children.push(subtree_one_side(
            target_file,
            target_transforms,
            t_child_tid,
            t_child_t,
            t_child_go,
            NodeStatus::Removed,
            &mut covered.target,
        )?);
    }

    let pruned = prune_unchanged(children);
    let status = aggregate_status(&pruned);

    let (id, target_id) = obj_id_pair(b_t.m_GameObject.m_PathID, t_t.m_GameObject.m_PathID);
    Ok(Node {
        children: pruned,
        target_id,
        ..make_node(id, go_label(b_go), "gameobject", status)
    })
}

/// One-side-only subtree (added on base, removed on target). Marks
/// the whole subtree with the same status and records all path-ids
/// as covered so loose doesn't pick them up again.
fn subtree_one_side<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    transforms: &Transforms,
    transform_path_id: PathId,
    transform: &Transform,
    go: &GameObject,
    status: NodeStatus,
    covered: &mut HashSet<PathId>,
) -> Result<Node> {
    covered.insert(transform_path_id);
    covered.insert(transform.m_GameObject.m_PathID);

    let mut children: Vec<Node> = Vec::new();
    // Components show up as same-status leaves; component-key lookup
    // would be wasted work since there's nothing to match against.
    let components = collect_components(file, go, transform_path_id, covered)?;
    for (key, comp) in &components {
        children.push(Node {
            badge: Some(format!("[{}]", comp.path_id)),
            facets: [("class".to_string(), key.to_string())]
                .into_iter()
                .collect(),
            ..make_node(
                format!("obj:{}", comp.path_id),
                component_label(file, key, comp)?,
                "component",
                status,
            )
        });
    }
    for child_pptr in &transform.m_Children {
        let child_path_id = child_pptr.m_PathID;
        if let Some((child_t, child_go)) = transforms.get(&child_path_id) {
            children.push(subtree_one_side(
                file,
                transforms,
                child_path_id,
                child_t,
                child_go,
                status,
                covered,
            )?);
        }
    }
    Ok(Node {
        children,
        ..make_node(
            format!("obj:{}", transform.m_GameObject.m_PathID),
            go_label(go),
            "gameobject",
            status,
        )
    })
}

fn go_label(go: &GameObject) -> String {
    if go.m_Name.is_empty() {
        "(unnamed)".to_string()
    } else {
        go.m_Name.clone()
    }
}

/// Children of a transform in their stored order, paired with each
/// child's own transform/go so callers don't have to re-deref.
fn ordered_children<'t>(
    transform: &Transform,
    transforms: &'t Transforms,
) -> Vec<(PathId, &'t Transform, &'t GameObject)> {
    let mut out = Vec::with_capacity(transform.m_Children.len());
    for child_pptr in &transform.m_Children {
        let id = child_pptr.m_PathID;
        if let Some((t, go)) = transforms.get(&id) {
            out.push((id, t, go));
        }
    }
    out
}

// ---- component matching --------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ComponentKey {
    /// MonoBehaviour, keyed by resolved `MonoScript::full_name()`.
    /// When the script name fails to resolve we drop down to ClassId
    /// so the row still pairs up.
    Script(String),
    /// Engine component, keyed by its `ClassId`.
    ClassId(ClassId),
}

impl std::fmt::Display for ComponentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentKey::Script(name) => f.write_str(name),
            ComponentKey::ClassId(class_id) => std::fmt::Debug::fmt(class_id, f),
        }
    }
}

struct Component {
    path_id: PathId,
}

fn collect_components<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    go: &GameObject,
    self_transform_pid: PathId,
    covered: &mut HashSet<PathId>,
) -> Result<BTreeMap<ComponentKey, Component>> {
    use serde_value::Value;
    let mut out: BTreeMap<ComponentKey, Component> = BTreeMap::new();
    for component in &go.m_Component {
        let component_ref = component
            .component
            .deref_local::<()>(file.file, &file.env.tpk)?;
        let class_id = component_ref.info.m_ClassID;
        let path_id = component_ref.info.m_PathID;
        covered.insert(path_id);
        // Skip the implicit self-transform — same convention as the
        // structured tree.
        if path_id == self_transform_pid
            && matches!(class_id, ClassId::Transform | ClassId::RectTransform)
        {
            continue;
        }
        let key = if class_id == ClassId::MonoBehaviour {
            let mb_obj = file.object_at::<Value>(path_id)?;
            let script = mb_obj.cast::<MonoBehaviour>().mono_script().ok().flatten();
            match script {
                Some(s) => ComponentKey::Script(s.full_name().into_owned()),
                None => ComponentKey::ClassId(class_id),
            }
        } else {
            ComponentKey::ClassId(class_id)
        };
        out.insert(key, Component { path_id });
    }
    Ok(out)
}

fn component_diff_node<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    target_file: &SerializedFileHandle<'_, R, P>,
    base_bodies: &BodyIndex<'_>,
    target_bodies: &BodyIndex<'_>,
    key: &ComponentKey,
    base: Option<&Component>,
    target: Option<&Component>,
) -> Result<Node> {
    match (base, target) {
        (Some(b), Some(t)) => {
            let status = match (base_bodies.get(&b.path_id), target_bodies.get(&t.path_id)) {
                (Some(bb), Some(tb)) if bb == tb => NodeStatus::Unchanged,
                _ => NodeStatus::Changed,
            };
            let (id, target_id) = obj_id_pair(b.path_id, t.path_id);
            Ok(Node {
                badge: if b.path_id == t.path_id {
                    Some(format!("[{}]", b.path_id))
                } else {
                    Some(format!("[{} → {}]", t.path_id, b.path_id))
                },
                facets: [("class".to_string(), key.to_string())]
                    .into_iter()
                    .collect(),
                target_id,
                ..make_node(id, component_label(base_file, key, b)?, "component", status)
            })
        }
        (Some(b), None) => Ok(Node {
            badge: Some(format!("[{}]", b.path_id)),
            facets: [("class".to_string(), key.to_string())]
                .into_iter()
                .collect(),
            ..make_node(
                format!("obj:{}", b.path_id),
                component_label(base_file, key, b)?,
                "component",
                NodeStatus::Added,
            )
        }),
        (None, Some(t)) => Ok(Node {
            badge: Some(format!("[{}]", t.path_id)),
            facets: [("class".to_string(), key.to_string())]
                .into_iter()
                .collect(),
            ..make_node(
                format!("obj:{}", t.path_id),
                component_label(target_file, key, t)?,
                "component",
                NodeStatus::Removed,
            )
        }),
        (None, None) => unreachable!(),
    }
}

fn component_label<R: EnvResolver, P: TypeTreeProvider>(
    _file: &SerializedFileHandle<'_, R, P>,
    key: &ComponentKey,
    _comp: &Component,
) -> Result<String> {
    Ok(key.to_string())
}

// ---- loose ---------------------------------------------------------------

/// Loose objects (everything not covered by the hierarchy walk).
/// Matched by `(class-or-script, m_Name)` for named assets so a
/// material that gained one byte but kept its name shows up as
/// `changed`; unnamed objects fall back to path-id (often a single
/// instance per class, so the join still does the right thing).
#[tracing::instrument(skip_all)]
fn diff_loose<R: EnvResolver, P: TypeTreeProvider>(
    base: &OpenedSide<R, P>,
    target: &OpenedSide<R, P>,
    covered: &Covered,
) -> Result<Node> {
    let base_file = load_file(base)?;
    let target_file = load_file(target)?;
    let base_bodies = build_body_index(&base_file);
    let target_bodies = build_body_index(&target_file);

    let base_raw = collect_loose(&base_file, &covered.base)?;
    let target_raw = collect_loose(&target_file, &covered.target)?;

    // A class that occurs exactly once on each side is conventionally
    // a singleton (RenderSettings, NavMeshSettings, LightmapSettings,
    // every `globalgamemanagers` entry, …). Pair those by class
    // alone so a path-id renumber or a missing `m_Name` doesn't make
    // them register as a removal + addition. Bucket by raw class id
    // (not the resolved label): `MonoBehaviour` is one class id for
    // every user script, so MBs never accidentally collapse to a
    // singleton even when the script name happens to be unique.
    let base_counts = class_id_counts(&base_raw);
    let target_counts = class_id_counts(&target_raw);
    let is_singleton = |class_id: ClassId| {
        base_counts.get(&class_id) == Some(&1) && target_counts.get(&class_id) == Some(&1)
    };
    let key_for = |raw: &RawLoose| LooseKey {
        label: raw.label.clone(),
        name: if is_singleton(raw.class_id) {
            String::new()
        } else if raw.name.is_empty() {
            format!("__pid:{}", raw.path_id)
        } else {
            raw.name.clone()
        },
    };
    let mut base_items: BTreeMap<LooseKey, LooseItem> = BTreeMap::new();
    for r in &base_raw {
        base_items.insert(key_for(r), LooseItem { path_id: r.path_id });
    }
    let mut target_items: BTreeMap<LooseKey, LooseItem> = BTreeMap::new();
    for r in &target_raw {
        target_items.insert(key_for(r), LooseItem { path_id: r.path_id });
    }

    let mut keys: Vec<LooseKey> = base_items
        .keys()
        .chain(target_items.keys())
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();

    let mut children: Vec<Node> = Vec::new();
    for key in keys {
        let b = base_items.remove(&key);
        let t = target_items.remove(&key);
        let node = match (b, t) {
            (Some(b), Some(t)) => {
                let status = match (base_bodies.get(&b.path_id), target_bodies.get(&t.path_id)) {
                    (Some(bb), Some(tb)) if bb == tb => NodeStatus::Unchanged,
                    _ => NodeStatus::Changed,
                };
                let (id, target_id) = obj_id_pair(b.path_id, t.path_id);
                Node {
                    badge: if b.path_id == t.path_id {
                        Some(format!("[{}]", b.path_id))
                    } else {
                        Some(format!("[{} → {}]", t.path_id, b.path_id))
                    },
                    facets: [("class".to_string(), key.label.clone())]
                        .into_iter()
                        .collect(),
                    target_id,
                    ..make_node(id, loose_label(&key, &b), "component", status)
                }
            }
            (Some(b), None) => Node {
                badge: Some(format!("[{}]", b.path_id)),
                facets: [("class".to_string(), key.label.clone())]
                    .into_iter()
                    .collect(),
                ..make_node(
                    format!("obj:{}", b.path_id),
                    loose_label(&key, &b),
                    "component",
                    NodeStatus::Added,
                )
            },
            (None, Some(t)) => Node {
                badge: Some(format!("[{}]", t.path_id)),
                facets: [("class".to_string(), key.label.clone())]
                    .into_iter()
                    .collect(),
                ..make_node(
                    format!("obj:{}", t.path_id),
                    loose_label(&key, &t),
                    "component",
                    NodeStatus::Removed,
                )
            },
            (None, None) => unreachable!(),
        };
        children.push(node);
    }

    let badge = Some(format!(
        "{} {}",
        children.len(),
        pluralize(children.len(), "object")
    ));
    let pruned = prune_unchanged(children);
    let status = aggregate_status(&pruned);
    Ok(Node {
        badge,
        children: pruned,
        ..make_node("section:loose", "Loose components", "section", status)
    })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LooseKey {
    /// `(class-or-script, m_Name)` for named items, with a synthetic
    /// `__pid:<n>` name for unnamed ones so the join falls back to
    /// per-side identity. Sorted lexicographically so the loose
    /// section is stable across runs.
    label: String,
    name: String,
}

struct LooseItem {
    path_id: PathId,
}

/// One row of a side's loose-object listing, before the per-side
/// data is folded together into [`LooseKey`]s. Held as a flat list
/// so [`diff_loose`] can count occurrences per `class_id` (to
/// detect singletons) before deciding how to key each item.
struct RawLoose {
    /// Raw Unity class id. Used as the singleton-detection axis —
    /// `RenderSettings` etc each have a unique class id, while every
    /// user script shares `ClassId::MonoBehaviour` so MBs never
    /// accidentally collapse to singletons.
    class_id: ClassId,
    /// User-facing class label: the raw class id for engine types,
    /// the resolved `MonoScript::full_name()` for MBs.
    label: String,
    name: String,
    path_id: PathId,
}

fn class_id_counts(items: &[RawLoose]) -> std::collections::HashMap<ClassId, usize> {
    let mut m: std::collections::HashMap<ClassId, usize> = std::collections::HashMap::new();
    for r in items {
        *m.entry(r.class_id).or_default() += 1;
    }
    m
}

#[tracing::instrument(skip_all)]
fn collect_loose<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    covered: &HashSet<PathId>,
) -> Result<Vec<RawLoose>> {
    use serde_value::Value;
    let mut out: Vec<RawLoose> = Vec::new();
    for obj in file.file.objects() {
        let path_id = obj.m_PathID;
        if covered.contains(&path_id) {
            continue;
        }
        let class_id = obj.m_ClassID;
        let _obj_span = tracing::info_span!("loose_object", ?class_id, path_id).entered();
        // Try to read `m_Name` for the typical asset shape. Failure
        // is OK — we fall back to a per-pid synthetic name later.
        let name = file
            .object_at::<Value>(path_id)
            .ok()
            .and_then(|h| h.read().ok())
            .and_then(|v| match v {
                Value::Map(map) => {
                    map.get(&Value::String("m_Name".to_string()))
                        .and_then(|n| match n {
                            Value::String(s) => Some(s.clone()),
                            _ => None,
                        })
                }
                _ => None,
            })
            .unwrap_or_default();
        let label = if class_id == ClassId::MonoBehaviour {
            file.object_at::<Value>(path_id)
                .ok()
                .and_then(|h| h.cast::<MonoBehaviour>().mono_script().ok().flatten())
                .map(|s| s.full_name().into_owned())
                .unwrap_or_else(|| format!("{class_id:?}"))
        } else {
            format!("{class_id:?}")
        };
        out.push(RawLoose {
            class_id,
            label,
            name,
            path_id,
        });
    }
    Ok(out)
}

fn loose_label(key: &LooseKey, _item: &LooseItem) -> String {
    if key.name.starts_with("__pid:") {
        key.label.clone()
    } else {
        format!("{}: {}", key.label, key.name)
    }
}

// ---- shared helpers ------------------------------------------------------

fn load_file<R: EnvResolver, P: TypeTreeProvider>(
    side: &OpenedSide<R, P>,
) -> Result<SerializedFileHandle<'_, R, P>> {
    Ok(side.env.load_cached(&side.relative)?)
}

fn object_bytes<'a, R: EnvResolver, P>(
    file: &SerializedFileHandle<'a, R, P>,
    obj: &ObjectInfo,
) -> &'a [u8] {
    let start = obj.m_Offset as usize;
    let end = start + obj.m_Size as usize;
    &file.data[start..end]
}

fn pluralize(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

fn prune_unchanged(mut children: Vec<Node>) -> Vec<Node> {
    children.retain(|c| c.status != Some(NodeStatus::Unchanged) || !c.children.is_empty());
    children
}

fn aggregate_status(children: &[Node]) -> NodeStatus {
    if children
        .iter()
        .any(|c| c.status != Some(NodeStatus::Unchanged))
    {
        NodeStatus::Changed
    } else {
        NodeStatus::Unchanged
    }
}
