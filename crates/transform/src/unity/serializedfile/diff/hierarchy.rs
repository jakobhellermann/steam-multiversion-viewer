use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::{GameObject, MonoBehaviour, Transform};

use crate::structured::{Node, NodeStatus};
use crate::unity::object_name;

use super::{BodyIndex, Side, go_label};

type Transforms = BTreeMap<PathId, (Transform, GameObject)>;

#[tracing::instrument(skip_all)]
pub(super) fn diff_hierarchy<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    target_file: &SerializedFileHandle<'_, R, P>,
) -> Result<(Node, Covered)> {
    let base_bodies = super::build_body_index(base_file);
    let target_bodies = super::build_body_index(target_file);

    let base_transforms = collect_transforms(base_file)?;
    let target_transforms = collect_transforms(target_file)?;

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
                        base_file,
                        target_file,
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
                        base_file,
                        &base_transforms,
                        b_id,
                        b_t,
                        b_go,
                        NodeStatus::Added,
                        Side::Base,
                        &mut covered.base,
                    )?);
                }
            }
        }
        for ti in unmatched_target {
            let (t_id, t_t, t_go) = target_roots[ti];
            children.push(subtree_one_side(
                target_file,
                &target_transforms,
                t_id,
                t_t,
                t_go,
                NodeStatus::Removed,
                Side::Target,
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
            super::pluralize(base_root_count, "root")
        ))
    } else {
        Some(format!(
            "{target_root_count} {} → {base_root_count} {}",
            super::pluralize(target_root_count, "root"),
            super::pluralize(base_root_count, "root")
        ))
    };

    let mut pruned = super::prune_unchanged(children);
    if pruned.len() == 1 {
        crate::unity::serializedfile::expand_single_child_chains(&mut pruned[0]);
    }
    let status = super::aggregate_status(&pruned);

    Ok((
        Node {
            badge,
            children: pruned,
            ..super::make_node("section:hierarchy", "Hierarchy", "section", status)
        },
        covered,
    ))
}

#[derive(Default)]
pub(super) struct Covered {
    pub(super) base: HashSet<PathId>,
    pub(super) target: HashSet<PathId>,
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

fn roots_with_data(transforms: &Transforms) -> Vec<(PathId, &Transform, &GameObject)> {
    transforms
        .iter()
        .filter(|(_, (t, _))| t.m_Father.is_null())
        .map(|(id, (t, go))| (*id, t, go))
        .collect()
}

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
    let mut matches: Vec<(usize, Option<usize>)> = Vec::with_capacity(base.len());
    for (bi, b) in base.iter().enumerate() {
        let k = key(b);
        let ti = target_keys
            .iter()
            .enumerate()
            .find(|(ti, tk)| **tk == k && !consumed.contains(ti))
            .map(|(ti, _)| ti);
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

    let base_components = collect_components(base_file, b_go, b_tid, &mut covered.base)?;
    let target_components = collect_components(target_file, t_go, t_tid, &mut covered.target)?;

    let mut keys: BTreeMap<(ComponentKey, usize), ()> = BTreeMap::new();
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
            &key.0,
            b,
            t,
        )?);
    }

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
                    Side::Base,
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
            Side::Target,
            &mut covered.target,
        )?);
    }

    let pruned = super::prune_unchanged(children);
    let status = super::aggregate_status(&pruned);

    let id = super::matched_pair_id(b_t.m_GameObject.m_PathID, t_t.m_GameObject.m_PathID);
    Ok(Node {
        children: pruned,
        ..super::make_node(id, go_label(b_go), "gameobject", status)
    })
}

#[allow(clippy::too_many_arguments)]
fn subtree_one_side<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    transforms: &Transforms,
    transform_path_id: PathId,
    transform: &Transform,
    go: &GameObject,
    status: NodeStatus,
    side: Side,
    covered: &mut HashSet<PathId>,
) -> Result<Node> {
    covered.insert(transform_path_id);
    covered.insert(transform.m_GameObject.m_PathID);

    let mut children: Vec<Node> = Vec::new();
    let components = collect_components(file, go, transform_path_id, covered)?;
    for ((key, _occurrence), comp) in &components {
        children.push(Node {
            badge: component_name(file, key, comp),
            facets: [("class".to_string(), key.to_string())]
                .into_iter()
                .collect(),
            ..super::make_node(
                super::one_sided_id(side, comp.path_id),
                key.to_string(),
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
                side,
                covered,
            )?);
        }
    }
    Ok(Node {
        children,
        ..super::make_node(
            super::one_sided_id(side, transform.m_GameObject.m_PathID),
            go_label(go),
            "gameobject",
            status,
        )
    })
}

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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ComponentKey {
    Script(String),
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
) -> Result<BTreeMap<(ComponentKey, usize), Component>> {
    use serde_value::Value;
    let mut out: BTreeMap<(ComponentKey, usize), Component> = BTreeMap::new();
    let mut occurrences: BTreeMap<ComponentKey, usize> = BTreeMap::new();
    for component in &go.m_Component {
        let component_ref = component
            .component
            .deref_local::<()>(file.file, &file.env.tpk)?;
        let class_id = component_ref.info.m_ClassID;
        let path_id = component_ref.info.m_PathID;
        covered.insert(path_id);
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
        let occ = occurrences.entry(key.clone()).or_insert(0);
        out.insert((key, *occ), Component { path_id });
        *occ += 1;
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
            let status = super::matched_status(
                base_file,
                target_file,
                base_bodies,
                target_bodies,
                b.path_id,
                t.path_id,
            );
            let id = super::matched_pair_id(b.path_id, t.path_id);
            Ok(Node {
                badge: component_name(base_file, key, b),
                facets: [("class".to_string(), key.to_string())]
                    .into_iter()
                    .collect(),
                ..super::make_node(id, key.to_string(), "component", status)
            })
        }
        (Some(b), None) => Ok(Node {
            badge: component_name(base_file, key, b),
            facets: [("class".to_string(), key.to_string())]
                .into_iter()
                .collect(),
            ..super::make_node(
                super::one_sided_id(Side::Base, b.path_id),
                key.to_string(),
                "component",
                NodeStatus::Added,
            )
        }),
        (None, Some(t)) => Ok(Node {
            badge: component_name(target_file, key, t),
            facets: [("class".to_string(), key.to_string())]
                .into_iter()
                .collect(),
            ..super::make_node(
                super::one_sided_id(Side::Target, t.path_id),
                key.to_string(),
                "component",
                NodeStatus::Removed,
            )
        }),
        (None, None) => unreachable!(),
    }
}

/// The component's name as badge (game-specific, else `m_Name`) —
/// engine classes carry no names. Matched pairs badge from the base
/// side.
fn component_name<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    key: &ComponentKey,
    comp: &Component,
) -> Option<String> {
    let ComponentKey::Script(script_name) = key else {
        return None;
    };
    object_name(file, script_name, comp.path_id)
}
